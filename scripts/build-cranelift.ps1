param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$CargoArgs
)

$ErrorActionPreference = "Stop"

$target = (& rustc +nightly -vV | Select-String '^host: ').ToString().Split(': ', 2)[1].Trim()

$supported = @(
    'x86_64-unknown-linux-gnu',
    'aarch64-unknown-linux-gnu',
    'x86_64-pc-windows-msvc',
    'aarch64-pc-windows-msvc',
    'x86_64-apple-darwin',
    'aarch64-apple-darwin'
)

if ($supported -notcontains $target) {
    Write-Error "cranelift backend is not supported for target $target; use the default LLVM backend"
}

$installed = & rustup component list --toolchain nightly 2>$null
if ($installed -notmatch '^rustc-codegen-cranelift') {
    & rustup component add rustc-codegen-cranelift --toolchain nightly
}

$env:RUSTFLAGS = "$(if ($env:RUSTFLAGS) { $env:RUSTFLAGS + ' ' })-Zcodegen-backend=cranelift"

& rustup run nightly cargo build --locked @CargoArgs
