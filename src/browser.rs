use anyhow::{Context, Result};
use async_channel::Sender;
use gpui::{App, Bounds, Pixels, Window};
use raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, WindowHandle};
use std::{cell::RefCell, rc::Rc};
use wry::{
    WebContext, WebView, WebViewBuilder,
    dpi::{LogicalPosition, LogicalSize},
};

pub type BrowserHandle = Rc<RefCell<Browser>>;

pub fn configure_renderer() {
    // GPUI 0.2.2 の DirectComposition target は子 HWND より手前に描画される。
    // HWND swapchain を使用して WebView2 が親の白背景に隠れるのを防ぐ。
    #[cfg(windows)]
    // SAFETY: Windows の環境変数 API はスレッドセーフ。GPUI 初期化前に適用する。
    unsafe {
        std::env::set_var("GPUI_DISABLE_DIRECT_COMPOSITION", "1");
    }
}

// UI スレッドで取得し、そのスレッド上の WebView2 初期化だけに使用する。
// Window の GPUI 借用を初期化中に保持しないためのネイティブハンドル。
struct ParentWindow(RawWindowHandle);
impl HasWindowHandle for ParentWindow {
    fn window_handle(&self) -> std::result::Result<WindowHandle<'_>, HandleError> {
        // SAFETY: GPUI が所有する親 HWND を同じ UI スレッドで使用する。
        // 初期化中のウィンドウ破棄は request_close により完了後まで延期する。
        Ok(unsafe { WindowHandle::borrow_raw(self.0) })
    }
}

#[derive(PartialEq, Eq)]
pub struct BrowserState {
    pub url: Option<String>,
    pub can_back: bool,
    pub can_forward: bool,
}

pub enum BrowserEvent {
    Page { url: String, loading: bool },
    Open(String),
    Ready,
    FocusList,
    ReaderFailed,
    Error(String),
}

// Readability.js と抽出後の表示処理を一つの即時関数にまとめ、ページのグローバルを汚さない。
const READER_SCRIPT: &str = concat!(
    "(() => {\n",
    include_str!("../assets/readability/Readability.js"),
    "\n",
    include_str!("../assets/reader.js"),
    "\n})();"
);

// ページ側が処理しなかった Esc だけを通知し、キー操作を一覧へ戻す。
const ESCAPE_SCRIPT: &str = "window.addEventListener('keydown', (event) => {
  if (event.key === 'Escape' && !event.defaultPrevented) window.ipc.postMessage('focus-list');
});";

#[derive(Clone, Copy)]
pub enum Command {
    Back,
    Forward,
    Reload,
}

pub struct Browser {
    // WebView を先に破棄し、WebContext を最後まで生存させる。
    view: Option<WebView>,
    context: Option<WebContext>,
    bounds: Option<Bounds<Pixels>>,
    scale_factor: f32,
    pending_url: Option<String>,
    initializing: bool,
    close_requested: bool,
    events: Sender<BrowserEvent>,
}

pub fn is_web_url(value: &str) -> bool {
    url::Url::parse(value)
        .is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some())
}

// WebView2 は Cookie・キャッシュの保存先を指定する。macOS の WKWebView は
// wry の保存先指定を使わず、OS 既定の保存先を使う。
fn profile_dir() -> Result<Option<std::path::PathBuf>> {
    if !cfg!(windows) {
        return Ok(None);
    }
    let profile = std::path::PathBuf::from(
        std::env::var_os("LOCALAPPDATA").context("LOCALAPPDATA が設定されていません")?,
    )
    .join("hatebu-reader")
    .join("WebView2");
    std::fs::create_dir_all(&profile).context("WebView の保存先を作成できません")?;
    Ok(Some(profile))
}

fn rect(bounds: Bounds<Pixels>) -> wry::Rect {
    wry::Rect {
        position: LogicalPosition::new(f64::from(bounds.origin.x), f64::from(bounds.origin.y))
            .into(),
        size: LogicalSize::new(f64::from(bounds.size.width), f64::from(bounds.size.height)).into(),
    }
}

impl Browser {
    pub fn new(events: Sender<BrowserEvent>) -> BrowserHandle {
        Rc::new(RefCell::new(Self {
            view: None,
            context: None,
            bounds: None,
            scale_factor: 1.,
            pending_url: None,
            initializing: false,
            close_requested: false,
            events,
        }))
    }

    pub fn sync_bounds(&mut self, bounds: Bounds<Pixels>, scale_factor: f32) {
        if self.bounds == Some(bounds) && self.scale_factor == scale_factor {
            return;
        }
        self.bounds = Some(bounds);
        self.scale_factor = scale_factor;
        if let Some(view) = &self.view
            && let Err(error) = view.set_bounds(rect(bounds))
        {
            let _ = self.events.try_send(BrowserEvent::Error(format!(
                "表示領域を更新できません: {error}"
            )));
        }
    }

    pub fn request_close(&mut self) -> bool {
        if self.initializing {
            self.close_requested = true;
            false
        } else {
            true
        }
    }

    pub fn take_close_request(&mut self) -> bool {
        !self.initializing && std::mem::take(&mut self.close_requested)
    }

    pub fn open(handle: &BrowserHandle, url: &str, window: &mut Window, cx: &mut App) {
        if !is_web_url(url) {
            return;
        }
        let mut browser = handle.borrow_mut();
        if let Some(view) = &browser.view {
            if let Err(error) = view.load_url(url) {
                let _ = browser
                    .events
                    .try_send(BrowserEvent::Error(format!("記事を開けません: {error}")));
            }
            return;
        }
        browser.pending_url = Some(url.to_owned());
        if browser.initializing {
            return;
        }
        let parent = match HasWindowHandle::window_handle(window) {
            Ok(handle) => ParentWindow(handle.as_raw()),
            Err(error) => {
                let _ = browser
                    .events
                    .try_send(BrowserEvent::Error(error.to_string()));
                return;
            }
        };
        browser.initializing = true;
        drop(browser);
        let close_handle = Rc::downgrade(handle);
        window.on_window_should_close(cx, move |_, _| {
            close_handle
                .upgrade()
                .is_none_or(|browser| browser.borrow_mut().request_close())
        });
        let handle = Rc::downgrade(handle);
        // WebView2 初期化はメッセージループへ再入する。GPUI の entity や
        // Browser を借用したまま初期化せず、描画処理からも分離する。
        cx.foreground_executor().spawn(async move {
            let Some(handle) = handle.upgrade() else { return; };
            let (bounds, url, events) = {
                let browser = handle.borrow();
                (browser.bounds, browser.pending_url.clone(), browser.events.clone())
            };
            let result = (|| -> Result<(WebView, WebContext)> {
                let bounds = bounds.context("記事の表示領域がまだありません")?;
                let url = url.as_deref().context("記事の URL がありません")?;
                let mut context = WebContext::new(profile_dir()?);
                let load_events = events.clone();
                let popup_events = events.clone();
                let download_events = events.clone();
                let ipc_events = events.clone();
                let view = WebViewBuilder::new_with_web_context(&mut context)
                    .with_bounds(rect(bounds))
                    .with_focused(false)
                    .with_url(url)
                    .with_initialization_script(ESCAPE_SCRIPT)
                    // 任意のページから届くため、フォーカス移動と帯の表示以外には使わない。
                    .with_ipc_handler(move |request| {
                        let event = match request.body().as_str() {
                            "focus-list" => BrowserEvent::FocusList,
                            "reader-failed" => BrowserEvent::ReaderFailed,
                            _ => return,
                        };
                        let _ = ipc_events.try_send(event);
                    })
                    .with_navigation_handler(|url| is_web_url(&url) || url == "about:blank")
                    .with_on_page_load_handler(move |event, url| {
                        let _ = load_events.try_send(BrowserEvent::Page {
                            url, loading: matches!(event, wry::PageLoadEvent::Started),
                        });
                    })
                    .with_new_window_req_handler(move |url, _| {
                        if is_web_url(&url) { let _ = popup_events.try_send(BrowserEvent::Open(url)); }
                        wry::NewWindowResponse::Deny
                    })
                    .with_download_started_handler(move |_, _| {
                        let _ = download_events.try_send(BrowserEvent::Error(
                            "ダウンロードは「ブラウザで開く」から行ってください。".into()));
                        false
                    })
                    .build_as_child(&parent)
                    .context(if cfg!(windows) {
                        "WebView2 を初期化できません。Microsoft Edge WebView2 Runtime を確認してください"
                    } else {
                        "記事の表示領域を初期化できません"
                    })?;
                Ok((view, context))
            })();
            let mut browser = handle.borrow_mut();
            browser.initializing = false;
            match result {
                Ok((view, context)) => {
                    // 初期化中に変わった領域・選択も反映する。
                    if let Some(bounds) = browser.bounds
                        && let Err(error) = view.set_bounds(rect(bounds)) {
                            let _ = events.try_send(BrowserEvent::Error(format!("表示領域を更新できません: {error}")));
                        }
                    if browser.pending_url != url
                        && let Some(url) = &browser.pending_url
                            && let Err(error) = view.load_url(url) {
                                let _ = events.try_send(BrowserEvent::Error(error.to_string()));
                            }
                    browser.view = Some(view);
                    browser.context = Some(context);
                    let _ = events.try_send(BrowserEvent::Ready);
                }
                Err(error) => { let _ = events.try_send(BrowserEvent::Error(format!("{error:#}"))); }
            }
        }).detach();
    }

    pub fn command(&self, command: Command) -> Result<()> {
        if let Some(view) = &self.view {
            match command {
                Command::Back => {
                    if view.can_go_back()? {
                        view.go_back()?;
                    }
                }
                Command::Forward => {
                    if view.can_go_forward()? {
                        view.go_forward()?;
                    }
                }
                Command::Reload => view.reload()?,
            }
        }
        Ok(())
    }

    pub fn focus_parent(&self) -> Result<()> {
        if let Some(view) = &self.view {
            view.focus_parent()?;
        }
        Ok(())
    }

    /// 記事を表示領域の高さ単位でスクロールする。負の値で上へ戻る。
    pub fn scroll_page(&self, pages: f64) -> Result<()> {
        if let Some(view) = &self.view {
            view.evaluate_script(&format!(
                "window.scrollBy({{ top: window.innerHeight * {}, behavior: 'smooth' }});",
                pages * 0.9
            ))?;
        }
        Ok(())
    }

    /// 表示中のページを本文だけの表示に置き換える。抽出できなければ ReaderFailed が届く。
    pub fn apply_reader(&self) -> Result<()> {
        if let Some(view) = &self.view {
            view.evaluate_script(READER_SCRIPT)?;
        }
        Ok(())
    }

    pub fn has_view(&self) -> bool {
        self.view.is_some()
    }

    pub fn can_go_back(&self) -> bool {
        self.view
            .as_ref()
            .is_some_and(|v| v.can_go_back().unwrap_or(false))
    }
    pub fn can_go_forward(&self) -> bool {
        self.view
            .as_ref()
            .is_some_and(|v| v.can_go_forward().unwrap_or(false))
    }
    pub fn current_url(&self) -> Option<String> {
        self.view
            .as_ref()
            .and_then(|v| v.url().ok())
            .filter(|url| is_web_url(url))
    }

    pub fn state(&self) -> BrowserState {
        BrowserState {
            url: self.current_url(),
            can_back: self.can_go_back(),
            can_forward: self.can_go_forward(),
        }
    }
}

// macOS の GPUI はメインスレッドでの起動を求めるため、cargo test のスレッドでは動かない。
#[cfg(all(test, windows))]
#[path = "browser_runtime_test.rs"]
mod runtime_test;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_only_http_articles() {
        assert!(is_web_url("https://example.com/a?x=1#section"));
        assert!(is_web_url("http://example.com"));
        for url in [
            "javascript:alert(1)",
            "file:///C:/test",
            "data:text/html,hi",
            "about:blank",
            "invalid",
        ] {
            assert!(!is_web_url(url), "{url}");
        }
    }
    #[test]
    fn converts_logical_bounds_without_applying_dpi_twice() {
        let actual = rect(Bounds::new(
            gpui::point(gpui::px(530.), gpui::px(112.)),
            gpui::size(gpui::px(700.), gpui::px(600.)),
        ));
        assert_eq!(
            actual.position.to_logical::<f64>(2.),
            LogicalPosition::new(530., 112.)
        );
        assert_eq!(
            actual.size.to_logical::<f64>(2.),
            LogicalSize::new(700., 600.)
        );
    }
}
