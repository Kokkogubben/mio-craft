# Packs the Store build (built with `--features store --no-bundle`) into an unsigned
# MSIX for Partner Center. The Store signs it on submission.
#
# Identity values come from Partner Center (Product identity page). Set them as
# repository variables MSIX_IDENTITY_NAME, MSIX_PUBLISHER and
# MSIX_PUBLISHER_DISPLAY_NAME; without them this makes a test package.
$ErrorActionPreference = "Stop"

$identity  = if ($env:MSIX_IDENTITY_NAME) { $env:MSIX_IDENTITY_NAME } else { "MioCraft.Dev" }
$publisher = if ($env:MSIX_PUBLISHER) { $env:MSIX_PUBLISHER } else { "CN=MioCraft Dev" }
$pubName   = if ($env:MSIX_PUBLISHER_DISPLAY_NAME) { $env:MSIX_PUBLISHER_DISPLAY_NAME } else { "MioCraft" }

# The Store wants four parts with the last one 0, e.g. 0.8.0 -> 0.8.0.0.
$conf = Get-Content src-tauri/tauri.conf.json -Raw | ConvertFrom-Json
$version = "$($conf.version).0"

$exe = "target/release/mio-craft.exe"
if (-not (Test-Path $exe)) { throw "$exe not found; build with: npm run tauri -- build --no-bundle --features store" }

$stage = "target/msix"
Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force "$stage/mcp", "$stage/Assets" | Out-Null
Copy-Item $exe $stage
Copy-Item mcp/dist/mio-craft-mcp.mjs "$stage/mcp/"
Copy-Item src-tauri/msix/Assets/* "$stage/Assets/"

$xml = Get-Content src-tauri/msix/AppxManifest.xml -Raw
$xml = $xml.Replace("__IDENTITY_NAME__", $identity).Replace("__PUBLISHER__", $publisher).Replace("__PUBLISHER_DISPLAY_NAME__", [Security.SecurityElement]::Escape($pubName)).Replace("__VERSION__", $version)
Set-Content "$stage/AppxManifest.xml" $xml -Encoding utf8

$makeappx = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\makeappx.exe" |
  Sort-Object { [version]$_.Directory.Parent.Name } -ErrorAction SilentlyContinue | Select-Object -Last 1
if (-not $makeappx) { throw "makeappx.exe not found (install the Windows SDK)" }

New-Item -ItemType Directory -Force target/msix-out | Out-Null
$out = "target/msix-out/MioCraft_${version}_x64.msix"
& $makeappx.FullName pack /d $stage /p $out /o
if ($LASTEXITCODE -ne 0) { throw "makeappx failed" }
Write-Host "Packed $out"
