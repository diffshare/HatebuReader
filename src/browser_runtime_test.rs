//! Windows の WebView2 Runtime と GPUI の実イベントループを使用する統合テスト。
use super::*;
use gpui::{AppContext, Application, BackgroundExecutor, WindowOptions, div};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

struct TestView;
impl gpui::Render for TestView {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        div()
    }
}

async fn loaded(
    events: &async_channel::Receiver<BrowserEvent>,
    url: &str,
    executor: &BackgroundExecutor,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        while let Ok(event) = events.try_recv() {
            match event {
                BrowserEvent::Error(error) => anyhow::bail!(error),
                BrowserEvent::Page {
                    url: actual,
                    loading: false,
                } if actual == url => return Ok(()),
                _ => {}
            }
        }
        executor.timer(Duration::from_millis(25)).await;
    }
    anyhow::bail!("WebView の読み込みがタイムアウトしました: {url}")
}

/// ページで式を評価し、JSON 文字列として結果を返す。
async fn evaluate(
    browser: &BrowserHandle,
    script: &str,
    executor: &BackgroundExecutor,
) -> Result<String> {
    let (tx, rx) = async_channel::bounded(1);
    browser
        .borrow()
        .view
        .as_ref()
        .context("WebView がありません")?
        .evaluate_script_with_callback(script, move |value| {
            let _ = tx.try_send(value);
        })?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Ok(value) = rx.try_recv() {
            return Ok(value);
        }
        executor.timer(Duration::from_millis(25)).await;
    }
    anyhow::bail!("ページの JavaScript を実行できませんでした")
}

async fn assert_document(
    browser: &BrowserHandle,
    expected: &str,
    executor: &BackgroundExecutor,
) -> Result<()> {
    let title = evaluate(browser, "document.title", executor).await?;
    anyhow::ensure!(
        title == format!("\"{expected}\""),
        "予期しない document.title: {title}"
    );
    Ok(())
}

async fn reader_failed(
    events: &async_channel::Receiver<BrowserEvent>,
    executor: &BackgroundExecutor,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        while let Ok(event) = events.try_recv() {
            if matches!(event, BrowserEvent::ReaderFailed) {
                return Ok(());
            }
        }
        executor.timer(Duration::from_millis(25)).await;
    }
    anyhow::bail!("本文の抽出失敗が通知されませんでした")
}

fn article_page() -> String {
    let paragraph = "<p>リーダー表示の統合テストに使う本文です。段落は十分な長さにして、抽出の対象になるようにしています。</p>";
    format!(
        "<!doctype html><html><head><title>記事の見出し</title></head><body><nav>メニュー</nav><article><h1>記事の見出し</h1>{}</article><aside>広告</aside></body></html>",
        paragraph.repeat(12)
    )
}

#[test]
#[ignore = "Windows の GPU と WebView2 Runtime を使用してウィンドウを開く統合テスト"]
fn webview_runtime() {
    configure_renderer();
    let server = TcpListener::bind("127.0.0.1:0").unwrap();
    server.set_nonblocking(true).unwrap();
    let base = format!("http://{}", server.local_addr().unwrap());
    let stop = Arc::new(AtomicBool::new(false));
    let server_stop = stop.clone();
    let server_thread = std::thread::spawn(move || {
        while !server_stop.load(Ordering::Relaxed) {
            match server.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                    let mut request = [0; 4096];
                    if let Ok(count) = stream.read(&mut request) {
                        let request = String::from_utf8_lossy(&request[..count]);
                        let (body, policy) = if request.starts_with("GET /article ") {
                            // リーダー表示の CSS が CSP の style-src で止められないことも確認する。
                            (
                                article_page(),
                                "Content-Security-Policy: style-src 'self'\r\n",
                            )
                        } else {
                            let title = if request.starts_with("GET /second ") {
                                "Second"
                            } else {
                                "First"
                            };
                            (
                                format!(
                                    "<!doctype html><html><head><title>{title}</title></head><body><h1>{title}</h1><input id='text'></body></html>"
                                ),
                                "",
                            )
                        };
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n{policy}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes());
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    });
    let outcome = Arc::new(Mutex::new(None));
    let task_outcome = outcome.clone();
    Application::new().run(move |cx| {
        let (tx, rx) = async_channel::unbounded();
        let browser = Browser::new(tx);
        let window = cx
            .open_window(WindowOptions::default(), |_, cx| cx.new(|_| TestView))
            .unwrap();
        cx.spawn(async move |cx| {
            let executor = cx.background_executor().clone();
            let result = async {
                let first = format!("{base}/first");
                let second = format!("{base}/second");
                let initial = Bounds::new(
                    gpui::point(gpui::px(10.), gpui::px(50.)),
                    gpui::size(gpui::px(480.), gpui::px(400.)),
                );
                window.update(cx, |_, window, cx| {
                    browser
                        .borrow_mut()
                        .sync_bounds(initial, window.scale_factor());
                    Browser::open(&browser, &first, window, cx);
                })?;
                loaded(&rx, &first, &executor).await?;
                assert_document(&browser, "First", &executor).await?;
                window.update(cx, |_, window, cx| {
                    Browser::open(&browser, &second, window, cx)
                })?;
                loaded(&rx, &second, &executor).await?;
                assert_document(&browser, "Second", &executor).await?;
                anyhow::ensure!(browser.borrow().can_go_back());
                browser.borrow().command(Command::Back)?;
                loaded(&rx, &first, &executor).await?;
                assert_document(&browser, "First", &executor).await?;
                anyhow::ensure!(browser.borrow().can_go_forward());
                browser.borrow().command(Command::Forward)?;
                loaded(&rx, &second, &executor).await?;
                assert_document(&browser, "Second", &executor).await?;
                browser.borrow().command(Command::Reload)?;
                loaded(&rx, &second, &executor).await?;
                assert_document(&browser, "Second", &executor).await?;
                let resized = Bounds::new(
                    gpui::point(gpui::px(20.), gpui::px(70.)),
                    gpui::size(gpui::px(360.), gpui::px(280.)),
                );
                let scale = window.update(cx, |_, window, _| window.scale_factor())?;
                browser.borrow_mut().sync_bounds(resized, scale);
                let actual = browser.borrow().view.as_ref().unwrap().bounds()?;
                anyhow::ensure!(
                    actual.size.to_logical::<f64>(f64::from(scale)) == LogicalSize::new(360., 280.)
                );
                let article = format!("{base}/article");
                window.update(cx, |_, window, cx| {
                    Browser::open(&browser, &article, window, cx)
                })?;
                loaded(&rx, &article, &executor).await?;
                browser.borrow().apply_reader()?;
                let state = evaluate(
                    &browser,
                    "[document.documentElement.dataset.bReader, document.adoptedStyleSheets.length, document.querySelector('.b-reader h1').textContent, document.querySelectorAll('nav, aside').length].join('|')",
                    &executor,
                )
                .await?;
                anyhow::ensure!(
                    state == "\"on|1|記事の見出し|0\"",
                    "リーダー表示になっていません: {state}"
                );
                window.update(cx, |_, window, cx| {
                    Browser::open(&browser, &first, window, cx)
                })?;
                loaded(&rx, &first, &executor).await?;
                browser.borrow().apply_reader()?;
                reader_failed(&rx, &executor).await?;
                Ok::<_, anyhow::Error>(())
            }
            .await;
            *task_outcome.lock().unwrap() = Some(result);
            let _ = window.update(cx, |_, window, _| window.remove_window());
            let _ = cx.update(|cx| cx.quit());
        })
        .detach();
    });
    stop.store(true, Ordering::Relaxed);
    server_thread.join().unwrap();
    outcome
        .lock()
        .unwrap()
        .take()
        .expect("統合テストが完了していません")
        .unwrap();
}
