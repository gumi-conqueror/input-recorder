param([string]$OutputDirectory = (Join-Path $PSScriptRoot '../dist'))

$ErrorActionPreference = 'Stop'
$projectDirectory = Split-Path $PSScriptRoot
Push-Location $projectDirectory
try {
    $metadataText = cargo metadata --locked --format-version 1 --filter-platform x86_64-pc-windows-msvc
    if ($LASTEXITCODE -ne 0) { throw 'Cargo metadata failed.' }
    $metadata = $metadataText | ConvertFrom-Json
    $packageDirectory = Join-Path $OutputDirectory 'input-recorder-windows-x64'
    New-Item -ItemType Directory -Force -Path $packageDirectory | Out-Null
    Copy-Item -LiteralPath (Join-Path $metadata.target_directory 'release/input-recorder.exe') -Destination $packageDirectory
    Copy-Item -LiteralPath 'LICENSE', 'README.md', 'docs/bundle-format.md' -Destination $packageDirectory

    $resolved = @{}
    foreach ($node in $metadata.resolve.nodes) { $resolved[$node.id] = $true }
    $noticesDirectory = Join-Path $packageDirectory 'third-party'
    New-Item -ItemType Directory -Force -Path $noticesDirectory | Out-Null
    $notices = foreach ($package in $metadata.packages | Sort-Object name, version) {
        if (!$resolved.ContainsKey($package.id) -or $package.id -eq $metadata.resolve.root) { continue }
        $crateDirectory = Split-Path $package.manifest_path
        $crateName = "$($package.name)-$($package.version)"
        $licenseFiles = @(Get-ChildItem -LiteralPath $crateDirectory -Recurse -File | Where-Object {
            $_.Name -match '(?i)license|licence|copying|copyright|^notice|^ofl\.|^ufl\.'
        })
        if ($licenseFiles.Count -eq 0) {
            $licenseFiles = @(Get-ChildItem -LiteralPath $crateDirectory -File -Filter 'README*')
        }
        $included = foreach ($file in $licenseFiles) {
            $relative = [IO.Path]::GetRelativePath($crateDirectory, $file.FullName)
            $destination = Join-Path (Join-Path $noticesDirectory $crateName) $relative
            New-Item -ItemType Directory -Force -Path (Split-Path $destination) | Out-Null
            Copy-Item -LiteralPath $file.FullName -Destination $destination
            $copied = Get-Item -LiteralPath $destination
            if ($copied.LastWriteTime.Year -lt 1980) { $copied.LastWriteTime = [datetime]'1980-01-01' }
            "$crateName/$($relative.Replace('\', '/'))"
        }
        [ordered]@{
            name = $package.name
            version = $package.version
            license = $package.license
            repository = $package.repository
            files = @($included)
        }
    }
    $notices | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $noticesDirectory 'index.json') -Encoding utf8
    $revision = git rev-parse HEAD
    if ($LASTEXITCODE -ne 0) { throw 'Cannot determine source revision.' }
    "Source: https://github.com/gumi-conqueror/input-recorder/tree/$revision" | Set-Content -LiteralPath (Join-Path $packageDirectory 'SOURCE.txt') -Encoding utf8
    $archive = Join-Path $OutputDirectory 'input-recorder-windows-x64.zip'
    Compress-Archive -LiteralPath $packageDirectory -DestinationPath $archive -Force
    Get-Item -LiteralPath $archive | Select-Object FullName, Length
} finally {
    Pop-Location
}
