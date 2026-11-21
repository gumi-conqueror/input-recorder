use std::{
    collections::{BTreeMap, BTreeSet},
    mem::{offset_of, size_of},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::Instant,
};

use anyhow::{Result, ensure};
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use serde_json::json;
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
        UI::{
            Input::{
                GetRawInputData, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RID_INPUT,
                RIDEV_DEVNOTIFY, RIDEV_INPUTSINK, RegisterRawInputDevices,
            },
            WindowsAndMessaging::{
                CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
                GIDC_REMOVAL, GWLP_USERDATA, GetMessageW, GetWindowLongPtrW, HWND_MESSAGE,
                KillTimer, MSG, PostQuitMessage, PostThreadMessageW, RegisterClassW, SetTimer,
                SetWindowLongPtrW, UnregisterClassW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_INPUT,
                WM_INPUT_DEVICE_CHANGE, WM_NCCREATE, WM_QUIT, WM_TIMER, WNDCLASSW,
            },
        },
    },
    core::w,
};

use crate::{
    devices,
    model::{Device, Event, Update},
    packet,
};

pub(crate) struct Capture {
    pub events: Receiver<Event>,
    pub updates: Receiver<Update>,
    pub lost: Arc<AtomicU64>,
    pub started: Instant,
    pub started_unix_us: u128,
    thread_id: u32,
    worker: Option<JoinHandle<()>>,
}

impl Capture {
    pub(crate) fn start() -> Result<Self> {
        let (event_tx, events) = bounded(16_384);
        let (update_tx, updates) = unbounded();
        let (ready_tx, ready_rx) = bounded(1);
        let lost = Arc::new(AtomicU64::new(0));
        let started = Instant::now();
        let started_unix_us = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_micros();
        let state = State {
            events: event_tx,
            updates: update_tx.clone(),
            lost: lost.clone(),
            devices: BTreeMap::new(),
            registered: BTreeSet::new(),
            started,
            sequence: 0,
            generation: 0,
            buffer: Vec::new(),
            last_error: String::new(),
        };
        let worker = thread::Builder::new()
            .name("raw-input".into())
            .spawn(move || {
                if let Err(error) = run(state, &ready_tx) {
                    let message = format!("capture stopped: {error:#}");
                    let _ = ready_tx.try_send(Err(message.clone()));
                    let _ = update_tx.send(Update::Diagnostic(message));
                }
            })?;
        let thread_id = ready_rx
            .recv()
            .map_err(|_| anyhow::anyhow!("capture initialization ended"))?
            .map_err(anyhow::Error::msg)?;
        Ok(Self {
            started,
            started_unix_us,
            events,
            updates,
            lost,
            thread_id,
            worker: Some(worker),
        })
    }

    pub(crate) fn stop(&mut self) -> Result<()> {
        if let Some(worker) = self.worker.take() {
            // SAFETY: thread_id belongs to our capture thread, whose message queue is ready.
            let posted =
                unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
            if posted.is_err() && !worker.is_finished() {
                self.worker = Some(worker);
                posted?;
            } else {
                worker
                    .join()
                    .map_err(|_| anyhow::anyhow!("capture thread panicked"))?;
            }
        }
        Ok(())
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        if self.worker.is_some() {
            // SAFETY: best-effort nonblocking shutdown of our message-loop thread.
            let _ = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        }
    }
}

struct State {
    events: Sender<Event>,
    updates: Sender<Update>,
    lost: Arc<AtomicU64>,
    devices: BTreeMap<usize, (Device, u64)>,
    registered: BTreeSet<(u16, u16)>,
    started: Instant,
    sequence: u64,
    generation: u64,
    buffer: Vec<usize>,
    last_error: String,
}

impl State {
    fn disconnect(&mut self, handle: usize) {
        if let Some((device, generation)) = self.devices.remove(&handle) {
            self.emit(
                &device.id,
                generation,
                "disconnected",
                json!({}),
                Vec::new(),
            );
        }
    }

    fn report_error(&mut self, error: String) {
        // Repeated malformed reports must not create an unbounded diagnostic stream.
        if self.last_error != error {
            let _ = self.updates.send(Update::Diagnostic(error.clone()));
            self.last_error = error;
        }
    }

    fn refresh(&mut self, window: HWND) -> Result<()> {
        let entries = devices::raw_list()?;
        let handles: BTreeSet<_> = entries
            .iter()
            .map(|entry| entry.hDevice.0 as usize)
            .collect();
        let removed: Vec<_> = self
            .devices
            .keys()
            .filter(|handle| !handles.contains(handle))
            .copied()
            .collect();
        let mut changed = !removed.is_empty();
        for handle in removed {
            self.disconnect(handle);
        }
        for entry in entries {
            let handle = entry.hDevice.0 as usize;
            if self.devices.contains_key(&handle) {
                continue;
            }
            let mut device = devices::describe(&entry);
            let usage = (device.usage_page, device.usage);
            if usage.0 != 0 && usage.1 != 0 && !self.registered.contains(&usage) {
                let registration = RAWINPUTDEVICE {
                    usUsagePage: usage.0,
                    usUsage: usage.1,
                    dwFlags: RIDEV_INPUTSINK | RIDEV_DEVNOTIFY,
                    hwndTarget: window,
                };
                // SAFETY: window is owned by this thread and remains live throughout registration.
                match unsafe {
                    RegisterRawInputDevices(&[registration], size_of::<RAWINPUTDEVICE>() as u32)
                } {
                    Ok(()) => {
                        self.registered.insert(usage);
                    }
                    Err(error) => device
                        .errors
                        .push(format!("Raw Input registration: {error}")),
                }
            }
            device.properties["raw_input_registered"] = json!(self.registered.contains(&usage));
            self.generation += 1;
            self.emit(
                &device.id,
                self.generation,
                "connected",
                json!({"device": &device}),
                Vec::new(),
            );
            self.devices.insert(handle, (device, self.generation));
            changed = true;
        }
        if changed {
            let _ = self.updates.send(Update::Devices(
                self.devices
                    .values()
                    .map(|(device, _)| device.clone())
                    .collect(),
            ));
        }
        Ok(())
    }

    fn emit(
        &mut self,
        id: &str,
        generation: u64,
        kind: &str,
        fields: serde_json::Value,
        raw: Vec<u8>,
    ) {
        self.sequence += 1;
        let event = Event {
            sequence: self.sequence,
            elapsed_us: self.started.elapsed().as_micros().min(u64::MAX as u128) as u64,
            device_id: id.into(),
            connection: generation,
            kind: kind.into(),
            fields,
            raw,
        };
        if self.events.try_send(event).is_err() {
            self.lost.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn input(&mut self, handle: HRAWINPUT) -> Result<()> {
        let received_at = self.started.elapsed().as_micros().min(u64::MAX as u128) as u64;
        let mut bytes = 0;
        // SAFETY: handle is supplied by the current WM_INPUT message and consumed before DefWindowProc.
        unsafe {
            ensure!(
                GetRawInputData(
                    handle,
                    RID_INPUT,
                    None,
                    &mut bytes,
                    size_of::<RAWINPUTHEADER>() as u32
                ) != u32::MAX,
                "Raw Input size query failed"
            );
            ensure!(
                (bytes as usize) >= size_of::<RAWINPUTHEADER>() && bytes <= 16 * 1024 * 1024,
                "invalid Raw Input size {bytes}"
            );
            self.buffer
                .resize((bytes as usize).div_ceil(size_of::<usize>()), 0);
            let received = GetRawInputData(
                handle,
                RID_INPUT,
                Some(self.buffer.as_mut_ptr().cast()),
                &mut bytes,
                size_of::<RAWINPUTHEADER>() as u32,
            );
            ensure!(
                received != u32::MAX
                    && received <= bytes
                    && received as usize >= size_of::<RAWINPUTHEADER>(),
                "Raw Input read failed"
            );
            let header = std::ptr::read_unaligned(self.buffer.as_ptr().cast::<RAWINPUTHEADER>());
            let offset = offset_of!(RAWINPUT, data);
            ensure!(
                header.dwSize as usize >= offset && header.dwSize <= received,
                "invalid Raw Input header"
            );
            let payload = std::slice::from_raw_parts(
                self.buffer.as_ptr().cast::<u8>().add(offset),
                header.dwSize as usize - offset,
            );
            let reports = packet::parse(header.dwType, payload)?;
            let (id, generation) = self
                .devices
                .get(&(header.hDevice.0 as usize))
                .map(|(device, generation)| (device.id.clone(), *generation))
                .unwrap_or_else(|| ("unattributed".into(), 0));
            for mut report in reports {
                report.fields["raw_device_handle"] =
                    json!(format!("{:x}", header.hDevice.0 as usize));
                self.sequence += 1;
                let event = Event {
                    sequence: self.sequence,
                    elapsed_us: received_at,
                    device_id: id.clone(),
                    connection: generation,
                    kind: report.kind.into(),
                    fields: report.fields,
                    raw: report.raw,
                };
                if self.events.try_send(event).is_err() {
                    self.lost.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        Ok(())
    }
}

fn run(state: State, ready: &Sender<std::result::Result<u32, String>>) -> Result<()> {
    let mut state = Box::new(state);
    // SAFETY: the boxed state stays at a stable address until after window destruction. All window work stays on this thread.
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: w!("InputRecorderRawInput"),
            ..Default::default()
        };
        ensure!(
            RegisterClassW(&class) != 0,
            "register capture window: {}",
            windows::core::Error::from_thread()
        );
        let window = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class.lpszClassName,
            w!(""),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance.into()),
            Some((&mut *state as *mut State).cast()),
        )?;
        let result = (|| -> Result<()> {
            state.refresh(window)?;
            ensure!(
                SetTimer(Some(window), 1, 2000, None) != 0,
                "create device discovery timer"
            );
            let _ = ready.send(Ok(GetCurrentThreadId()));
            let mut message = MSG::default();
            loop {
                let status = GetMessageW(&mut message, None, 0, 0).0;
                if status == 0 {
                    break;
                }
                ensure!(status != -1, "capture message loop failed");
                DispatchMessageW(&message);
            }
            Ok(())
        })();
        let _ = KillTimer(Some(window), 1);
        let _ = DestroyWindow(window);
        let _ = UnregisterClassW(class.lpszClassName, Some(instance.into()));
        result
    }
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: Windows supplies valid message parameters; WM_NCCREATE contains our live boxed State pointer.
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(lparam.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(window, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        let pointer = GetWindowLongPtrW(window, GWLP_USERDATA) as *mut State;
        if !pointer.is_null() {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let state = &mut *pointer;
                let result = match message {
                    WM_INPUT => state.input(HRAWINPUT(lparam.0 as *mut _)),
                    WM_INPUT_DEVICE_CHANGE => {
                        if wparam.0 == GIDC_REMOVAL as usize {
                            // Evict immediately: a later arrival can reuse the same handle.
                            state.disconnect(lparam.0 as usize);
                        }
                        state.refresh(window)
                    }
                    WM_TIMER => state.refresh(window),
                    _ => Ok(()),
                };
                if let Err(error) = result {
                    state.lost.fetch_add(1, Ordering::Relaxed);
                    state.report_error(format!("{error:#}"));
                }
            }));
            if result.is_err() {
                (*pointer)
                    .report_error("capture callback panicked; recording is incomplete".into());
                (*pointer).lost.fetch_add(1, Ordering::Relaxed);
                PostQuitMessage(1);
            }
        }
        // WM_INPUT cleanup must happen even for packets that could not be parsed.
        DefWindowProcW(window, message, wparam, lparam)
    }
}
