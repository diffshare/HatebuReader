fn main() {
    println!("cargo:rerun-if-changed=assets/icon/icon.ico");
    // Windows の実行ファイルにアイコンを埋め込む。GPUI はリソース ID 1 のアイコンを
    // ウィンドウにも使うため、タスクバーにも同じアイコンが出る。
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("assets/icon/icon.ico");
        resource
            .compile()
            .expect("実行ファイルにアイコンを埋め込めません");
    }
}
