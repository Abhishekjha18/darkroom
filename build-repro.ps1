# Reproducible build - the declared bonus (+5). See docs/BUILD.md section 3.
#
# Builds twice from a deleted release/ directory and compares SHA-256.
# Both halves matter:
#   - CARGO_ENCODED_RUSTFLAGS, not RUSTFLAGS: this project's path contains a
#     space, and RUSTFLAGS is whitespace-split with no quoting.
#   - rm -r release/, not `cargo clean --release`: clean can leave the binary
#     in place, and then the second "build" is a no-op whose hash trivially
#     matches. That comparison passes forever and proves nothing.

$ErrorActionPreference = "Stop"

$project = $PSScriptRoot
$toolchain = "1.97.1-x86_64-pc-windows-gnu"
$target = "x86_64-pc-windows-gnu"

$env:RUSTUP_HOME = "D:\Aniket\rust\.rustup"
$env:CARGO_HOME = "D:\Aniket\rust\.cargo"
$env:CARGO_TARGET_DIR = "D:\Aniket\rust\tmp\target"
$env:CARGO_INCREMENTAL = "0"
$env:SOURCE_DATE_EPOCH = "1000000000"

# RUSTFLAGS would be split on the space in "Zero Dependency". This will not.
Remove-Item Env:\RUSTFLAGS -ErrorAction SilentlyContinue
$US = [char]0x1f
$env:CARGO_ENCODED_RUSTFLAGS =
  "--remap-path-prefix=$project=." + $US + "-Clink-arg=-Wl,--no-insert-timestamp"

$cargo = "$env:CARGO_HOME\bin\cargo.exe"
$releaseDir = "$env:CARGO_TARGET_DIR\$target\release"
$exe = "$releaseDir\darkroom.exe"

$hashes = @()
foreach ($i in 1, 2) {
    Remove-Item -Recurse -Force $releaseDir -ErrorAction SilentlyContinue
    if (Test-Path $exe) { throw "could not remove $exe - a previous run may still be executing it" }

    & $cargo "+$toolchain" build --release --target $target | Out-Null
    if (-not (Test-Path $exe)) { throw "build $i produced no binary" }

    $h = (Get-FileHash $exe -Algorithm SHA256).Hash
    $hashes += $h
    Write-Host ("build {0}  {1}" -f $i, $h)
}

if ($hashes[0] -eq $hashes[1]) {
    Write-Host ""
    Write-Host "REPRODUCIBLE - byte-identical across two full rebuilds"
    Write-Host "  toolchain  $toolchain"
    Write-Host "  target     $target"
    Write-Host "  sha256     $($hashes[0])"
    exit 0
}

Write-Host ""
Write-Host "NOT REPRODUCIBLE - the two builds differ"
exit 1
