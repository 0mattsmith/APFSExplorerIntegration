fn main() {
    // Required by winfsp-rs with the `delayload` feature: emits the
    // delay-load linker flags for winfsp-x64.dll so the binary starts even
    // before WinFsp is loaded, then binds at first use.
    winfsp::build::winfsp_link_delayload();
}
