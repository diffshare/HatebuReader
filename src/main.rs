mod browser;
mod feed;

use browser::{Browser, BrowserEvent, BrowserHandle, Command};
use feed::{CATEGORIES, Entry, FeedKind};
use gpui::{div, prelude::*, px, rgb, *};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// はてなの案内に従い、同じ RSS への自動の取得は 30 分に 1 回程度までにする。
const FEED_CACHE_TTL: Duration = Duration::from_secs(30 * 60);

const INK: u32 = 0x1d2733;
const TITLE: u32 = 0x17202b;
const MUTED: u32 = 0x5b6675;
const READ: u32 = 0x5f6a78;
const BODY: u32 = 0x46515f;
const ICON: u32 = 0x3d4856;
const DISABLED: u32 = 0xb4bcc6;
const BORDER: u32 = 0xe1e5ea;
const RULE: u32 = 0xedf0f3;
const RAIL: u32 = 0xf0f2f5;
const SURFACE: u32 = 0xf6f7f9;
const ACCENT: u32 = 0x0a64a4;
const SELECTED: u32 = 0xe6f0f8;
const DANGER: u32 = 0xa3213f;
const WARN_BG: u32 = 0xfff4e5;
const WARN_FG: u32 = 0x6b4100;
const UI_FONT: &str = if cfg!(target_os = "macos") {
    "Hiragino Sans"
} else {
    "Yu Gothic UI"
};

// アイコンは OS のフォントに依存しないよう SVG を実行ファイルに埋め込む。
const ICONS: [(&str, &[u8]); 8] = [
    ("icons/book.svg", include_bytes!("../assets/icons/book.svg")),
    ("icons/back.svg", include_bytes!("../assets/icons/back.svg")),
    (
        "icons/forward.svg",
        include_bytes!("../assets/icons/forward.svg"),
    ),
    (
        "icons/reload.svg",
        include_bytes!("../assets/icons/reload.svg"),
    ),
    (
        "icons/open-external.svg",
        include_bytes!("../assets/icons/open-external.svg"),
    ),
    (
        "icons/comment.svg",
        include_bytes!("../assets/icons/comment.svg"),
    ),
    (
        "icons/error.svg",
        include_bytes!("../assets/icons/error.svg"),
    ),
    (
        "icons/document.svg",
        include_bytes!("../assets/icons/document.svg"),
    ),
];

struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        Ok(ICONS
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, bytes)| (*bytes).into()))
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(ICONS
            .iter()
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| (*name).into())
            .collect())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReaderPage {
    Article,
    Comments,
}

struct BookmarkApp {
    kind: FeedKind,
    category: usize,
    entries: Vec<Entry>,
    loading: bool,
    error: Option<String>,
    updated: Option<SystemTime>,
    load_task: Option<Task<()>>,
    // RSS の URL ごとの取得結果と取得時刻。カテゴリの切り替えで使い回す。
    feed_cache: HashMap<String, (Vec<Entry>, Instant)>,
    // コメント表示中も選択を保つため、表示中の URL とは別に持つ。
    selected: Option<Entry>,
    read: HashSet<String>,
    page: ReaderPage,
    focus: FocusHandle,
    list_scroll: ScrollHandle,
    browser: BrowserHandle,
    browser_loading: bool,
    browser_error: Option<String>,
    current_url: Option<String>,
    // リーダー表示の設定（記事を移っても続ける）と、表示中のページで抽出に失敗したか。
    reader: bool,
    reader_failed: bool,
    _browser_events: Task<()>,
    _browser_state: Task<()>,
}

impl BookmarkApp {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (events, receiver) = async_channel::unbounded();
        let browser_events = cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = receiver.recv().await {
                if this
                    .update_in(cx, |app, window, cx| {
                        match event {
                            BrowserEvent::Page { url, loading } => {
                                if browser::is_web_url(&url) {
                                    app.current_url = Some(url);
                                }
                                app.browser_loading = loading;
                                if loading {
                                    app.reader_failed = false;
                                } else if app.reader && app.page == ReaderPage::Article {
                                    app.apply_reader();
                                }
                            }
                            BrowserEvent::Open(url) => app.open_article(&url, window, cx),
                            BrowserEvent::Ready => {
                                if let Some(url) = app.browser.borrow().current_url() {
                                    app.current_url = Some(url);
                                }
                            }
                            BrowserEvent::FocusList => app.focus_list(window),
                            // 任意のページから届き得るため、リーダー表示中だけ扱う。
                            BrowserEvent::ReaderFailed => app.reader_failed = app.reader,
                            BrowserEvent::Error(error) => {
                                app.browser_error = Some(error);
                                app.browser_loading = false;
                            }
                        }
                        if app.browser.borrow_mut().take_close_request() {
                            window.remove_window();
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        // pushState やアンカー移動では page-load が発生しないため、ネイティブ
        // WebView の URL と履歴状態を確認し、変化したときだけ UI を更新する。
        let browser_state = cx.spawn(async move |this, cx| {
            let mut previous = None;
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                if this
                    .update(cx, |app, cx| {
                        let state = app.browser.borrow().state();
                        if previous.as_ref() != Some(&state) {
                            if let Some(url) = &state.url {
                                app.current_url = Some(url.clone());
                            }
                            previous = Some(state);
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let browser = Browser::new(events);
        let focus = cx.focus_handle();
        window.focus(&focus);
        let mut app = Self {
            kind: FeedKind::Popular,
            category: 0,
            entries: Vec::new(),
            loading: false,
            error: None,
            updated: None,
            load_task: None,
            feed_cache: HashMap::new(),
            selected: None,
            read: HashSet::new(),
            page: ReaderPage::Article,
            focus,
            list_scroll: ScrollHandle::new(),
            browser,
            browser_loading: false,
            browser_error: None,
            current_url: None,
            reader: false,
            reader_failed: false,
            _browser_events: browser_events,
            _browser_state: browser_state,
        };
        app.reload(cx);
        app
    }

    /// 表示中の RSS を取得し直す。利用者の再読み込みは記憶した一覧より優先する。
    fn reload(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        self.error = None;
        let url = self.kind.url(self.category);
        let fetch = cx.background_executor().spawn({
            let url = url.clone();
            async move { feed::fetch_feed(&url) }
        });
        // 前の受信タスクを破棄し、古いカテゴリの結果による上書きを防ぐ。
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let result = fetch.await;
            let _ = this.update(cx, |app, cx| {
                app.loading = false;
                match result {
                    Ok(entries) => {
                        app.feed_cache
                            .insert(url, (entries.clone(), Instant::now()));
                        app.entries = entries;
                        app.updated = Some(SystemTime::now());
                    }
                    Err(error) => app.error = Some(format!("{error:#}")),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn select(&mut self, kind: FeedKind, category: usize, cx: &mut Context<Self>) {
        if self.kind == kind && self.category == category {
            return;
        }
        self.kind = kind;
        self.category = category;
        if let Some((entries, fetched)) = self.feed_cache.get(&kind.url(category))
            && is_fresh(*fetched, Instant::now())
        {
            // 取得中の別カテゴリの結果で上書きされないよう、受信タスクも破棄する。
            self.load_task = None;
            self.loading = false;
            self.error = None;
            self.entries = entries.clone();
            self.updated = Some(SystemTime::now() - fetched.elapsed());
            cx.notify();
            return;
        }
        self.entries.clear();
        self.updated = None;
        self.reload(cx);
    }

    fn open_article(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !browser::is_web_url(url) {
            return;
        }
        self.current_url = Some(url.to_owned());
        self.browser_loading = true;
        self.browser_error = None;
        Browser::open(&self.browser, url, window, cx);
        cx.notify();
    }

    fn open_entry(&mut self, entry: Entry, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.entries.iter().position(|e| e.url == entry.url) {
            self.list_scroll.scroll_to_item(index);
        }
        let url = entry.url.clone();
        self.read.insert(url.clone());
        self.page = ReaderPage::Article;
        self.selected = Some(entry);
        self.open_article(&url, window, cx);
    }

    fn step(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let current = self
            .selected
            .as_ref()
            .and_then(|s| self.entries.iter().position(|e| e.url == s.url));
        let index = match current {
            Some(index) => index
                .saturating_add_signed(delta)
                .min(self.entries.len().saturating_sub(1)),
            None => 0,
        };
        if current != Some(index)
            && let Some(entry) = self.entries.get(index).cloned()
        {
            self.open_entry(entry, window, cx);
        }
    }

    fn show_page(&mut self, page: ReaderPage, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = &self.selected else {
            return;
        };
        let url = match page {
            ReaderPage::Article => entry.url.clone(),
            ReaderPage::Comments => entry.comments_url.clone(),
        };
        self.page = page;
        self.open_article(&url, window, cx);
    }

    fn open_external(&self, cx: &mut Context<Self>) {
        let url = self
            .browser
            .borrow()
            .current_url()
            .or_else(|| self.current_url.clone());
        if let Some(url) = url {
            cx.open_url(&url);
        }
    }

    // GPUI はクリックで子 HWND からフォーカスを取り戻さないため、明示的に戻す。
    fn focus_list(&mut self, window: &mut Window) {
        if let Err(error) = self.browser.borrow().focus_parent() {
            self.browser_error = Some(format!("一覧に戻れません: {error:#}"));
        }
        window.focus(&self.focus);
    }

    fn browser_command(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        self.browser_error = None;
        // 初期化失敗後も再読み込みから初期化をやり直せる。
        if matches!(command, Command::Reload)
            && let Some(url) = self.current_url.clone()
            && self.browser.borrow().current_url().is_none()
        {
            self.open_article(&url, window, cx);
            return;
        }
        if let Err(error) = self.browser.borrow().command(command) {
            self.browser_error = Some(format!("記事を操作できません: {error:#}"));
        }
        cx.notify();
    }

    fn can_use_reader(&self) -> bool {
        self.selected.is_some()
            && self.page == ReaderPage::Article
            && self.browser.borrow().has_view()
    }

    /// 画面上でリーダー表示になっているか（抽出に失敗したページではオフに見せる）。
    fn reader_shown(&self) -> bool {
        self.reader && !self.reader_failed && self.can_use_reader()
    }

    fn apply_reader(&mut self) {
        if let Err(error) = self.browser.borrow().apply_reader() {
            self.browser_error = Some(format!("リーダー表示にできません: {error:#}"));
        }
    }

    fn toggle_reader(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_use_reader() {
            return;
        }
        if self.reader_shown() {
            // 差し替えた本文を戻すより、元のページを読み込み直す方が確実。
            self.reader = false;
            self.browser_command(Command::Reload, window, cx);
        } else {
            self.reader = true;
            self.reader_failed = false;
            if !self.browser_loading {
                self.apply_reader();
            }
        }
        cx.notify();
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = &keystroke.modifiers;
        // Shift+スペースで上へ戻れるよう、スペースだけは Shift を許可する。
        if keystroke.key == "space"
            && !(modifiers.control || modifiers.alt || modifiers.platform || modifiers.function)
        {
            let direction = if modifiers.shift { -1. } else { 1. };
            if let Err(error) = self.browser.borrow().scroll_page(direction) {
                self.browser_error = Some(format!("記事をスクロールできません: {error:#}"));
                cx.notify();
            }
            return;
        }
        // r はリーダー表示に使うため、一覧の再読み込みは Shift+R にする。
        if keystroke.key == "r"
            && modifiers.shift
            && !(modifiers.control || modifiers.alt || modifiers.platform || modifiers.function)
        {
            if !self.loading {
                self.reload(cx);
            }
            return;
        }
        if modifiers.modified() {
            return;
        }
        match keystroke.key.as_str() {
            "j" => self.step(1, window, cx),
            "k" => self.step(-1, window, cx),
            "h" if self.category > 0 => self.select(self.kind, self.category - 1, cx),
            "l" if self.category + 1 < CATEGORIES.len() => {
                self.select(self.kind, self.category + 1, cx)
            }
            "t" => {
                let kind = match self.kind {
                    FeedKind::Popular => FeedKind::Recent,
                    FeedKind::Recent => FeedKind::Popular,
                };
                self.select(kind, self.category, cx);
            }
            "c" => {
                let page = match self.page {
                    ReaderPage::Article => ReaderPage::Comments,
                    ReaderPage::Comments => ReaderPage::Article,
                };
                self.show_page(page, window, cx);
            }
            "o" => self.open_external(cx),
            "r" => self.toggle_reader(window, cx),
            key => {
                if let Ok(number @ 1..=9) = key.parse::<usize>() {
                    self.select(self.kind, number - 1, cx);
                }
            }
        }
    }

    fn rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut kinds = div().flex().gap_1().p_1().rounded_lg().bg(rgb(0xe2e6eb));
        for (index, kind) in [FeedKind::Popular, FeedKind::Recent]
            .into_iter()
            .enumerate()
        {
            let active = self.kind == kind;
            kinds = kinds.child(
                div()
                    .id(("kind", index))
                    .flex_1()
                    .h(px(36.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .font_weight(FontWeight::SEMIBOLD)
                    .cursor_pointer()
                    .text_color(rgb(if active { TITLE } else { BODY }))
                    .when(active, |el| el.bg(rgb(0xffffff)).shadow_sm())
                    .on_click(cx.listener(move |app, _, _, cx| app.select(kind, app.category, cx)))
                    .child(kind.label()),
            );
        }
        let mut categories = div().flex().flex_col().gap(px(2.)).child(
            div()
                .px(px(10.))
                .pb_1()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(MUTED))
                .child("カテゴリ"),
        );
        for (index, (name, _)) in CATEGORIES.iter().enumerate() {
            let active = index == self.category;
            categories = categories.child(
                div()
                    .id(("category", index))
                    .h(px(40.))
                    .px_3()
                    .flex()
                    .items_center()
                    .justify_between()
                    .rounded_md()
                    .cursor_pointer()
                    .text_color(rgb(if active { ACCENT } else { 0x2c3643 }))
                    .font_weight(if active {
                        FontWeight::BOLD
                    } else {
                        FontWeight::MEDIUM
                    })
                    .when(active, |el| el.bg(rgb(0xffffff)))
                    .when(!active, |el| el.hover(|s| s.bg(rgb(0xe6e9ee))))
                    .on_click(cx.listener(move |app, _, _, cx| app.select(app.kind, index, cx)))
                    .child(*name)
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::NORMAL)
                            .text_color(rgb(MUTED))
                            .child((index + 1).to_string()),
                    ),
            );
        }
        let mut keys = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .p_3()
            .rounded_lg()
            .bg(rgb(0xe6e9ee))
            .text_xs()
            .text_color(rgb(ICON))
            .child(div().font_weight(FontWeight::SEMIBOLD).child("キーボード"));
        for (key, label) in [
            ("j k", "前後の記事"),
            ("Space", "スクロール"),
            ("1〜9", "カテゴリを選ぶ"),
            ("h l", "前後のカテゴリ"),
            ("t", "人気 / 新着"),
            ("Esc", "一覧に戻る"),
            ("c", "コメント"),
            ("o", "ブラウザで開く"),
            ("r", "リーダー"),
            ("R", "再読み込み"),
        ] {
            keys = keys.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .w(px(52.))
                            .flex_shrink_0()
                            .flex()
                            .gap_1()
                            .children(key.split(' ').map(kbd)),
                    )
                    .child(div().flex_1().min_w_0().child(label)),
            );
        }
        div()
            .id("rail")
            .w(px(184.))
            .flex_shrink_0()
            .h_full()
            .overflow_y_scroll()
            .px_3()
            .py_5()
            .flex()
            .flex_col()
            .gap_5()
            .bg(rgb(RAIL))
            .border_r_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .px_2()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .child("Hatebu Reader"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child("はてなブックマーク・非公式"),
                    ),
            )
            .child(kinds)
            .child(categories)
            .child(div().mt_auto().child(keys))
    }

    fn entry_row(
        &self,
        index: usize,
        entry: &Entry,
        now: SystemTime,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = self.selected.as_ref().is_some_and(|s| s.url == entry.url);
        let read = self.read.contains(&entry.url);
        let (badge_bg, badge_fg) = match entry.count {
            200.. => (0xb8264a, 0xffffff),
            100.. => (0xfbe7ec, DANGER),
            _ => (0xeef1f4, BODY),
        };
        let age = relative_age(&entry.date, now).unwrap_or_else(|| {
            entry
                .date
                .replace('T', " ")
                .trim_end_matches('Z')
                .to_owned()
        });
        let target = entry.clone();
        div()
            .id(("entry", index))
            .flex()
            .gap(px(14.))
            .items_start()
            .px_5()
            .py(px(14.))
            .border_b_1()
            .border_color(rgb(RULE))
            .cursor_pointer()
            .bg(rgb(if selected { SELECTED } else { 0xffffff }))
            .when(!selected, |el| el.hover(|s| s.bg(rgb(0xf6f8fa))))
            .on_click(
                cx.listener(move |app, _, window, cx| app.open_entry(target.clone(), window, cx)),
            )
            .child(
                div()
                    .w(px(52.))
                    .h(px(46.))
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .rounded_lg()
                    .bg(rgb(badge_bg))
                    .text_color(rgb(badge_fg))
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::BOLD)
                            .child(entry.count.to_string()),
                    )
                    .child(div().text_size(px(10.)).child("users")),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(14.5))
                            .font_weight(if read && !selected {
                                FontWeight::MEDIUM
                            } else {
                                FontWeight::SEMIBOLD
                            })
                            .text_color(rgb(if selected {
                                ACCENT
                            } else if read {
                                READ
                            } else {
                                TITLE
                            }))
                            .line_clamp(2)
                            .text_ellipsis()
                            .child(entry.title.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child(entry.domain.clone())
                            .child("·")
                            .child(age),
                    ),
            )
    }

    fn list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let subtitle = if self.loading && self.entries.is_empty() {
            format!("{}エントリー・取得中…", self.kind.label())
        } else if self.updated.is_some() {
            format!("{}エントリー・{} 件", self.kind.label(), self.entries.len())
        } else {
            format!("{}エントリー・未取得", self.kind.label())
        };
        let header = div()
            .relative()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_3()
            .pl_5()
            .pr_3()
            .py_4()
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .child(CATEGORIES[self.category].0),
                    )
                    .child(div().text_xs().text_color(rgb(MUTED)).child(subtitle)),
            )
            .child(
                icon_button("reload-list", "icons/reload.svg", !self.loading, false)
                    .border_1()
                    .border_color(rgb(0xd5dbe2))
                    .on_click(cx.listener(|app, _, _, cx| {
                        if !app.loading {
                            app.reload(cx);
                        }
                    })),
            )
            .when(self.loading, |el| el.child(progress_bar("list-progress")));
        let mut body = div()
            .id("entries")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.list_scroll)
            .flex()
            .flex_col();
        if self.entries.is_empty() {
            body = body.child(if self.loading {
                skeleton().into_any_element()
            } else if let Some(error) = &self.error {
                notice(
                    "icons/error.svg",
                    DANGER,
                    "一覧を取得できませんでした",
                    Some(error.clone()),
                )
                .child(primary_button("retry-list", "再試行").on_click(cx.listener(
                    |app, _, _, cx| {
                        if !app.loading {
                            app.reload(cx);
                        }
                    },
                )))
                .into_any_element()
            } else {
                notice(
                    "icons/document.svg",
                    MUTED,
                    "エントリーがありません",
                    Some("時間をおいて再読み込みしてください。".into()),
                )
                .when(self.kind == FeedKind::Popular, |el| {
                    el.child(secondary_button("show-recent", "新着を見る").on_click(
                        cx.listener(|app, _, _, cx| app.select(FeedKind::Recent, app.category, cx)),
                    ))
                })
                .into_any_element()
            });
        }
        let now = SystemTime::now();
        for (index, entry) in self.entries.iter().enumerate() {
            body = body.child(self.entry_row(index, entry, now, cx));
        }
        div()
            .w(px(360.))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(0xffffff))
            .border_r_1()
            .border_color(rgb(BORDER))
            .child(header)
            .when_some(
                self.error.as_ref().filter(|_| !self.entries.is_empty()),
                |el, error| {
                    el.child(
                        banner(
                            "更新できませんでした。前回取得した一覧を表示しています。",
                            error.clone(),
                        )
                        .child(
                            div()
                                .id("retry-stale")
                                .flex_shrink_0()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .font_weight(FontWeight::BOLD)
                                .cursor_pointer()
                                .hover(|s| s.bg(rgb(0xfbe6c4)))
                                .on_click(cx.listener(|app, _, _, cx| {
                                    if !app.loading {
                                        app.reload(cx);
                                    }
                                }))
                                .child("再試行"),
                        ),
                    )
                },
            )
            .child(body)
    }

    fn reader(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let can_back = self.browser.borrow().can_go_back();
        let can_forward = self.browser.borrow().can_go_forward();
        let has_article = self.current_url.is_some();
        let has_view = self.browser.borrow().has_view();
        let mut toolbar = div()
            .relative()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_2()
            .px_4()
            .py_3()
            .bg(rgb(0xffffff))
            .border_b_1()
            .border_color(rgb(BORDER));
        for (id, path, command, enabled) in [
            ("back", "icons/back.svg", Command::Back, can_back),
            (
                "forward",
                "icons/forward.svg",
                Command::Forward,
                can_forward,
            ),
            (
                "reload-page",
                "icons/reload.svg",
                Command::Reload,
                has_article,
            ),
        ] {
            toolbar = toolbar.child(icon_button(id, path, enabled, false).on_click(cx.listener(
                move |app, _, window, cx| {
                    if app.current_url.is_some() {
                        app.browser_command(command, window, cx);
                    }
                },
            )));
        }
        let address = match self
            .current_url
            .as_deref()
            .and_then(|u| url::Url::parse(u).ok())
        {
            Some(url) => {
                let rest = &url[url::Position::BeforePath..];
                div()
                    .flex()
                    .min_w_0()
                    .overflow_hidden()
                    .child(
                        div()
                            .flex_shrink_0()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(url.host_str().unwrap_or_default().to_owned()),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .text_color(rgb(MUTED))
                            .text_ellipsis()
                            .child(rest.to_owned()),
                    )
            }
            None => div().text_color(rgb(MUTED)).child("記事を選択してください"),
        };
        toolbar = toolbar.child(
            div()
                .flex_1()
                .min_w_0()
                .h(px(36.))
                .px(px(14.))
                .flex()
                .items_center()
                .rounded_full()
                .bg(rgb(RAIL))
                .text_sm()
                .whitespace_nowrap()
                .overflow_hidden()
                .child(address),
        );
        if let Some(entry) = &self.selected {
            let mut pages = div()
                .flex_shrink_0()
                .flex()
                .gap(px(2.))
                .p(px(3.))
                .rounded_lg()
                .bg(rgb(RAIL));
            for page in [ReaderPage::Article, ReaderPage::Comments] {
                let active = self.page == page;
                let id = match page {
                    ReaderPage::Article => "page-article",
                    ReaderPage::Comments => "page-comments",
                };
                pages = pages.child(
                    div()
                        .id(id)
                        .h(px(30.))
                        .px_3()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .rounded_md()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .cursor_pointer()
                        .text_color(rgb(if active { TITLE } else { BODY }))
                        .when(active, |el| el.bg(rgb(0xffffff)).shadow_sm())
                        .on_click(
                            cx.listener(move |app, _, window, cx| app.show_page(page, window, cx)),
                        )
                        .map(|el| match page {
                            ReaderPage::Article => el.child("記事"),
                            ReaderPage::Comments => {
                                el.child(icon(
                                    "icons/comment.svg",
                                    14.,
                                    if active { TITLE } else { BODY },
                                ))
                                .child("コメント")
                                .child(div().text_color(rgb(DANGER)).child(entry.count.to_string()))
                            }
                        }),
                );
            }
            toolbar = toolbar.child(pages);
        }
        let reader_shown = self.reader_shown();
        toolbar = toolbar
            .when(self.selected.is_some(), |el| {
                el.child(
                    icon_button(
                        "reader",
                        "icons/book.svg",
                        self.can_use_reader(),
                        reader_shown,
                    )
                    .when(!reader_shown, |el| {
                        el.border_1().border_color(rgb(0xd5dbe2))
                    })
                    .on_click(cx.listener(|app, _, window, cx| app.toggle_reader(window, cx))),
                )
            })
            .child(
                icon_button(
                    "open-external",
                    "icons/open-external.svg",
                    has_article,
                    false,
                )
                .border_1()
                .border_color(rgb(0xd5dbe2))
                .on_click(cx.listener(|app, _, _, cx| app.open_external(cx))),
            )
            .when(self.browser_loading, |el| {
                el.child(progress_bar("page-progress"))
            });
        // WebView が手前に重なるため、WebView があるときのエラーは帯で表示する。
        let init_failed = has_article && !has_view && self.browser_error.is_some();
        let browser = self.browser.clone();
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(SURFACE))
            .child(toolbar)
            .when_some(
                self.browser_error.as_ref().filter(|_| !init_failed),
                |el, error| {
                    el.child(
                        div()
                            .flex_shrink_0()
                            .px_4()
                            .py_2()
                            .bg(rgb(0xfbe9ec))
                            .border_b_1()
                            .border_color(rgb(0xf1c9d1))
                            .text_sm()
                            .text_color(rgb(DANGER))
                            .child(error.clone()),
                    )
                },
            )
            .when(self.reader && self.reader_failed, |el| {
                el.child(banner(
                    "本文を抽出できませんでした。",
                    "元のページを表示しています。".into(),
                ))
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .when(!has_article, |el| {
                        el.child(
                            notice(
                                "icons/document.svg",
                                MUTED,
                                "記事を選ぶと、ここに表示します",
                                None,
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .text_sm()
                                    .text_color(rgb(BODY))
                                    .child("一覧から選ぶか、")
                                    .child(kbd("j"))
                                    .child(kbd("k"))
                                    .child("キーで前後の記事に移れます。"),
                            ),
                        )
                    })
                    .when(init_failed, |el| {
                        el.child(
                            notice(
                                "icons/error.svg",
                                DANGER,
                                "記事を表示できません",
                                self.browser_error.clone(),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(primary_button("retry-browser", "再試行").on_click(
                                        cx.listener(|app, _, window, cx| {
                                            app.browser_command(Command::Reload, window, cx)
                                        }),
                                    ))
                                    .child(
                                        secondary_button(
                                            "open-external-fallback",
                                            "ブラウザで開く",
                                        )
                                        .on_click(
                                            cx.listener(|app, _, _, cx| app.open_external(cx)),
                                        ),
                                    ),
                            ),
                        )
                    })
                    .child(
                        canvas(
                            move |bounds, window, _| {
                                browser
                                    .borrow_mut()
                                    .sync_bounds(bounds, window.scale_factor());
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    ),
            )
    }
}

impl Render for BookmarkApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .bg(rgb(0xffffff))
            .text_color(rgb(INK))
            .font_family(UI_FONT)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|app, _, window, _| app.focus_list(window)),
            )
            .child(self.rail(cx))
            .child(self.list(cx))
            .child(self.reader(cx))
    }
}

// SVG は自身の文字色で塗るため、親の色を継承しない。
fn icon(path: &'static str, size: f32, color: u32) -> Svg {
    svg()
        .path(path)
        .size(px(size))
        .flex_shrink_0()
        .text_color(rgb(color))
}

/// `pressed` はオン・オフを切り替えるボタンのオンの見た目。
fn icon_button(
    id: &'static str,
    path: &'static str,
    enabled: bool,
    pressed: bool,
) -> Stateful<Div> {
    let color = match (enabled, pressed) {
        (false, _) => DISABLED,
        (true, true) => ACCENT,
        (true, false) => ICON,
    };
    div()
        .id(id)
        .size(px(36.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded_lg()
        .when(enabled && !pressed, |el| {
            el.cursor_pointer().hover(|s| s.bg(rgb(0xeef1f4)))
        })
        .when(enabled && pressed, |el| {
            el.cursor_pointer()
                .bg(rgb(SELECTED))
                .border_1()
                .border_color(rgb(ACCENT))
        })
        .child(icon(path, 14., color))
}

fn primary_button(id: &'static str, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(40.))
        .px_5()
        .flex()
        .items_center()
        .rounded_lg()
        .bg(rgb(ACCENT))
        .text_color(rgb(0xffffff))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .hover(|s| s.bg(rgb(0x084e80)))
        .child(label)
}

fn secondary_button(id: &'static str, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(40.))
        .px_5()
        .flex()
        .items_center()
        .rounded_lg()
        .border_1()
        .border_color(rgb(0xd5dbe2))
        .bg(rgb(0xffffff))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .hover(|s| s.bg(rgb(0xf3f5f8)))
        .child(label)
}

fn kbd(key: &str) -> Div {
    div()
        .px(px(5.))
        .rounded_sm()
        .border_1()
        .border_color(rgb(0xc4cbd4))
        .bg(rgb(0xffffff))
        .text_xs()
        .text_color(rgb(ICON))
        .child(key.to_owned())
}

/// 状態の説明を中央に置く。操作ボタンは呼び出し側で追加する。
fn notice(icon_path: &'static str, color: u32, title: &'static str, detail: Option<String>) -> Div {
    div()
        .size_full()
        .p_8()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_3()
        .child(icon(icon_path, 32., color))
        .child(div().text_base().font_weight(FontWeight::BOLD).child(title))
        .when_some(detail, |el, detail| {
            el.child(
                div()
                    .max_w(px(420.))
                    .text_sm()
                    .text_color(rgb(BODY))
                    .text_center()
                    .child(detail),
            )
        })
}

fn banner(message: &'static str, detail: String) -> Div {
    div()
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap_2()
        .pl_5()
        .pr_3()
        .py_2()
        .bg(rgb(WARN_BG))
        .border_b_1()
        .border_color(rgb(0xf3dfc2))
        .text_sm()
        .text_color(rgb(WARN_FG))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(message)
                .child(div().text_xs().line_clamp(2).text_ellipsis().child(detail)),
        )
}

fn skeleton() -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .children([0.94, 0.88, 0.96, 0.82, 0.9, 0.86].map(|width| {
            div()
                .flex()
                .gap(px(14.))
                .px_5()
                .py(px(14.))
                .border_b_1()
                .border_color(rgb(RULE))
                .child(
                    div()
                        .w(px(52.))
                        .h(px(46.))
                        .flex_shrink_0()
                        .rounded_lg()
                        .bg(rgb(0xeef1f4)),
                )
                .child(
                    div()
                        .flex_1()
                        .pt_1()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .h(px(12.))
                                .w(relative(width))
                                .rounded_md()
                                .bg(rgb(0xe8ebef)),
                        )
                        .child(
                            div()
                                .h(px(12.))
                                .w(relative(width - 0.3))
                                .rounded_md()
                                .bg(rgb(0xe8ebef)),
                        )
                        .child(
                            div()
                                .h(px(10.))
                                .w(relative(0.36))
                                .rounded_md()
                                .bg(rgb(RAIL)),
                        ),
                )
        }))
}

fn progress_bar(id: &'static str) -> impl IntoElement {
    div()
        .absolute()
        .left_0()
        .right_0()
        .bottom_0()
        .h(px(3.))
        .overflow_hidden()
        .bg(rgb(0xdbe7f2))
        .child(
            div()
                .absolute()
                .top_0()
                .h_full()
                .w(relative(0.35))
                .bg(rgb(ACCENT))
                .with_animation(
                    id,
                    Animation::new(Duration::from_millis(1200)).repeat(),
                    |bar, delta| bar.left(relative(delta * 1.35 - 0.35)),
                ),
        )
}

/// RSS の `YYYY-MM-DDTHH:MM:SSZ` を Unix 秒に変換する。
fn is_fresh(fetched: Instant, now: Instant) -> bool {
    now.saturating_duration_since(fetched) < FEED_CACHE_TTL
}

fn unix_seconds(date: &str) -> Option<i64> {
    let bytes = date.as_bytes();
    if bytes.len() != 20
        || [
            (4, b'-'),
            (7, b'-'),
            (10, b'T'),
            (13, b':'),
            (16, b':'),
            (19, b'Z'),
        ]
        .iter()
        .any(|&(index, byte)| bytes[index] != byte)
    {
        return None;
    }
    let number = |range: std::ops::Range<usize>| {
        let text = date.get(range)?;
        text.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| text.parse::<i64>().ok())
            .flatten()
    };
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    // 暦日から 1970-01-01 起点の日数を求める（days from civil）。
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second)
}

fn relative_age(date: &str, now: SystemTime) -> Option<String> {
    let now = i64::try_from(now.duration_since(UNIX_EPOCH).ok()?.as_secs()).ok()?;
    let seconds = now - unix_seconds(date)?;
    Some(match seconds {
        ..60 => "たった今".to_owned(),
        60..3_600 => format!("{}分前", seconds / 60),
        3_600..86_400 => format!("{}時間前", seconds / 3_600),
        _ => format!("{}日前", seconds / 86_400),
    })
}

fn main() {
    browser::configure_renderer();
    if std::env::args().any(|arg| arg == "--check-feeds") {
        let result = (|| -> anyhow::Result<()> {
            for (category, (name, _)) in CATEGORIES.iter().enumerate() {
                for kind in [FeedKind::Popular, FeedKind::Recent] {
                    let url = kind.url(category);
                    let entries = feed::fetch_feed(&url)?;
                    println!(
                        "{} / {}: {} entries ({url})",
                        name,
                        kind.label(),
                        entries.len()
                    );
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("{error:#}");
            std::process::exit(1);
        }
        return;
    }
    Application::new().with_assets(Assets).run(|cx: &mut App| {
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let bounds = Bounds::centered(None, size(px(1440.), px(900.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(1040.), px(560.))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Hatebu Reader".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| BookmarkApp::new(window, cx)),
        )
        .expect("ウィンドウを作成できません");
        cx.activate(true);
    });
}

#[cfg(test)]
mod tests {
    // `use super::*` だと gpui の `test` 属性マクロが `#[test]` を上書きする。
    use super::{FEED_CACHE_TTL, is_fresh, relative_age, unix_seconds};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    fn at(date: &str) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(unix_seconds(date).unwrap() as u64)
    }

    #[test]
    fn reuses_feeds_only_within_thirty_minutes() {
        let fetched = Instant::now();
        assert!(is_fresh(fetched, fetched));
        assert!(is_fresh(
            fetched,
            fetched + FEED_CACHE_TTL - Duration::from_secs(1)
        ));
        assert!(!is_fresh(fetched, fetched + FEED_CACHE_TTL));
    }

    #[test]
    fn converts_rss_dates_to_unix_seconds() {
        assert_eq!(unix_seconds("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(unix_seconds("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(unix_seconds("2026-11-08T08:17:47Z"), Some(1_794_125_867));
        for invalid in [
            "",
            "2026-11-08 08:17:47Z",
            "2026-13-08T08:17:47Z",
            "2026-11-08T08:17:47+09:00",
            "2026-11-0aT08:17:47Z",
            "２０２６-11-08T08:17:47Z",
        ] {
            assert_eq!(unix_seconds(invalid), None, "{invalid}");
        }
    }

    #[test]
    fn formats_relative_age() {
        let now = at("2026-11-09T10:18:00Z");
        assert_eq!(
            relative_age("2026-11-09T10:17:30Z", now).as_deref(),
            Some("たった今")
        );
        assert_eq!(
            relative_age("2026-11-09T10:00:00Z", now).as_deref(),
            Some("18分前")
        );
        assert_eq!(
            relative_age("2026-11-09T00:08:28Z", now).as_deref(),
            Some("10時間前")
        );
        assert_eq!(
            relative_age("2026-11-08T08:17:47Z", now).as_deref(),
            Some("1日前")
        );
        assert_eq!(relative_age("invalid", now), None);
    }
}
