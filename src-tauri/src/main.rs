// 发布版不弹出命令行窗口（Windows）
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    mailbeacon_app_lib::run()
}
