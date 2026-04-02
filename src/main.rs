#[cfg(target_os = "linux")]
fn configure_linux_window_backend() {
    if std::env::var_os("DISPLAY").is_some() && std::env::var_os("WAYLAND_DISPLAY").is_some() {
        std::env::remove_var("WAYLAND_DISPLAY");
        std::env::remove_var("WAYLAND_SOCKET");
        std::env::set_var("WINIT_UNIX_BACKEND", "x11");
    }
}

#[cfg(not(target_os = "linux"))]
fn configure_linux_window_backend() {}

fn main() {
    configure_linux_window_backend();

    if let Err(error) = spektar_ui::run_app() {
        eprintln!("failed to start Spektar: {error}");
        std::process::exit(1);
    }
}
