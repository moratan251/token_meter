// リリースビルドで Windows のコンソールウィンドウが余計に開かないようにする。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    token_meter_lib::run()
}
