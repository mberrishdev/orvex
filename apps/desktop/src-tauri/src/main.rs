// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // The installed desktop executable also serves MCP without opening a window.
    if std::env::args().nth(1).as_deref() == Some("mcp") {
        std::process::exit(orvex_desktop_lib::cli_main());
    }
    orvex_desktop_lib::run()
}
