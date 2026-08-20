#[cfg(windows)]
pub fn use_utf8_output() {
    use windows::Win32::System::Console::SetConsoleOutputCP;

    const UTF8_CODE_PAGE: u32 = 65001;

    unsafe {
        let _ = SetConsoleOutputCP(UTF8_CODE_PAGE);
    }
}

#[cfg(not(windows))]
pub fn use_utf8_output() {}
