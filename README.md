# Hatebu Reader

はてなブックマークの人気・新着エントリーを読むための、Windows・macOS 向けデスクトップアプリです。[GPUI](https://crates.io/crates/gpui) で作っています。

株式会社はてなとは関係のない非公式のアプリです。開発者は GitHub の [diffshare](https://github.com/diffshare) です。不具合や要望は [Issues](https://github.com/diffshare/HatebuReader/issues) にお寄せください。

## 機能

- 人気・新着エントリーの切替
- 総合と 8 カテゴリの閲覧
- 記事タイトル・ブックマーク数・ドメイン・公開からの経過時間の表示
- 開いた記事は一覧で既読として薄く表示（アプリの起動中のみ記憶）
- 記事を選択すると右側の WebView（Windows は WebView2、macOS は WKWebView）にページを表示
- 広告やメニューを除いて本文だけを読むリーダー表示（記事を移っても続ける）
- 記事とはてなブックマークのコメント一覧を、同じ表示領域で切り替え
- 記事の戻る・進む・再読み込み・ブラウザで開く
- 取得中の表示、通信エラーの表示と再試行（更新に失敗しても前回の一覧を残す）
- キーボード操作（下記）

ログイン不要の公開 RSS を使用します。ブックマークの追加・編集、記事のオフライン保存は実装していません。件数と並び順は RSS の配信内容に従います。既読・カテゴリの選択・リーダー表示の設定は保存せず、起動のたびに「総合・人気」の通常表示から始まります。

## キーボード操作

| キー | 動作 |
|---|---|
| `j` / `k` | 次・前の記事を開く |
| `Space` / `Shift`+`Space` | 記事を 1 画面分下・上へスクロール |
| `1`〜`9` | カテゴリを選ぶ（総合 = 1 … アニメとゲーム = 9） |
| `h` / `l` | 前後のカテゴリ |
| `t` | 人気と新着の切替 |
| `c` | 記事とコメントの切替 |
| `o` | 表示中のページをブラウザで開く |
| `r` | リーダー表示の切替 |
| `Shift`+`r` | 一覧を再読み込み |
| `Esc` | 記事ページから一覧へキー操作を戻す |

記事ページをクリックするとキー入力はページ側へ渡ります。一覧に戻すには `Esc` を押すか、一覧やカテゴリをクリックします。ページ自身が `Esc` を使う場合（ダイアログを閉じるなど）はページ側の処理を優先します。

## 入手

[Releases](https://github.com/diffshare/HatebuReader/releases) で、Windows（x64）と macOS（Apple Silicon）向けの zip を配布します。zip には、このアプリと同梱ライブラリのライセンス（`LICENSE`、`THIRD-PARTY-LICENSES.html`、`Readability-LICENSE.md`）も入っています。

どちらも開発者の署名をしていないため、初回の起動時に OS の警告が出ます。

- Windows：「Windows によって PC が保護されました」と出たら、「詳細情報」から「実行」を選びます。
- macOS：`Hatebu Reader.app` を「アプリケーション」に移して開き、開けないと表示されたら「システム設定」の「プライバシーとセキュリティ」で「このまま開く」を選びます。

## ソースからビルド

必要なもの：

- Rust stable（edition 2024 に対応したもの）
- Windows：Visual Studio 2022 の C++ ビルドツールと Windows SDK、Microsoft Edge WebView2 Runtime（Windows 11 には標準で入っています）
- macOS：Xcode Command Line Tools（Xcode 本体は不要）
- GPUI は GPU を使用するため、対応する GPU とドライバー

リポジトリのフォルダーで次のコマンドを実行します。

```sh
cargo run --locked --release
```

macOS では GPUI の `runtime_shaders` 機能を有効にし、Metal シェーダーを起動時にコンパイルします。そのため Xcode 本体がなくてもビルドできます。

## RSS の取得

はてなの[過剰アクセスに関する注意事項](https://b.hatena.ne.jp/help/entry/notice_access)に従い、同じ RSS フィードを取得してから 30 分以内は、カテゴリや人気・新着を切り替えても取得し直さず、前回の一覧を表示します。最新の一覧が必要なときは `Shift`+`r` か一覧の再読み込みボタンで取得します。

## 記事の表示

一覧の記事をクリックするとアプリ内にサイト本来のページを読み込みます。カテゴリを切り替えても表示中の記事は維持します。ページ内の別ウィンドウ向けリンクは同じ WebView に開きます。ダウンロードは「ブラウザで開く」から行ってください。

各ページには、`Esc` を一覧へ通知するための短いスクリプトを読み込み時に追加します。通知を受けたアプリは、キー入力の行き先を一覧へ戻す以外の処理をしません。

Windows では WebView の Cookie・キャッシュを `%LOCALAPPDATA%\hatebu-reader\WebView2` に保存します。既定のブラウザーとは別のプロファイルです。macOS では WKWebView の既定の保存先を使います。初期化に失敗した場合はエラーを表示し、記事側の「再読み込み」で再試行できます。

GPUI 0.2.2 の DirectComposition は子ウィンドウより手前に描画されるため、Windows では起動時に `GPUI_DISABLE_DIRECT_COMPOSITION=1` をプロセス内で設定し、HWND swapchain を使用します。

### リーダー表示

ツールバーの本のアイコンか `r` で切り替えます。ページの読み込みが終わると、Mozilla の [Readability.js](https://github.com/mozilla/readability)（0.6.0、`assets/readability/`）で表示中のページから本文を取り出し、サイト名・読む時間の目安・タイトル・本文だけの表示に置き換えます。そのため、読み込み中は元のページが一瞬見えます。オフにするとページを読み込み直します。

- オンのまま `j` / `k` で記事を移ると、次の記事もリーダー表示にします。
- 本文を取り出せないページ（本文が 200 文字未満など）は元のページのまま、帯で知らせます。次のページでは再び試します。
- はてなのコメント一覧では使えません。
- ページの CSP で止められないよう、スタイルは `<style>` を挿入せず CSSOM（`adoptedStyleSheets`）で当てます。

## 開発

```sh
cargo test --locked
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo run --locked -- --check-feeds
```

`--check-feeds` はウィンドウを開かず、全 18 フィードを 1 回ずつ取得して解析します。ネットワーク接続が必要です。取得失敗時は終了コード 1 を返します。

GPUI と WebView2 の実イベントループを使用する統合テストは、Windows で次のコマンドで実行します。ローカルの HTTP テストページを使用し、記事読み込み・JavaScript 実行・戻る・進む・再読み込み・表示領域の変更・リーダー表示（CSP 付きのページでの適用と、抽出できないページの通知）を検証します。ウィンドウを開くため、通常のテストからは除外しています。macOS の GPUI はメインスレッドでの起動を求めるため、`cargo test` からは実行できず対象外です。

```sh
cargo test --locked webview_runtime -- --ignored --test-threads=1
```

### CI とリリース

- `.github/workflows/ci.yml`：PR と main への push で、Windows と macOS の書式チェック・clippy・テスト・ビルドを行います。WebView2 の統合テストは Windows で試行し、失敗しても全体は止めません。`actions.lock` の検証も行います。
- `.github/workflows/release.yml`：`v` で始まるタグ（例：`v0.1.0`）を push すると、配布用の zip を作って下書きのリリースに添付します。タグは `Cargo.toml` の `version` と一致させます。配布の設定を変える PR と手動実行では、zip を作って Actions の成果物に置くだけです。
- 依存ライブラリのライセンス一覧は [cargo-about](https://github.com/EmbarkStudios/cargo-about) で作ります（`about.toml`、`about.hbs`）。`about.toml` で許可していないライセンスの依存が入ると失敗します。
- 外部の Action は GitHub の[ワークフローのロックファイル](https://docs.github.com/en/actions/concepts/security/workflow-dependency-lockfiles)（`.github/workflows/actions.lock`）で固定します。ワークフローの `uses:` を変えたら `gh actions-lock` で作り直します。更新は Dependabot が PR で知らせます。

### 構成

- `src/main.rs`: GPUI のウィンドウ、一覧、操作、非同期取得状態
- `src/feed.rs`: RSS の URL、HTTP 取得、XML 解析、解析テスト
- `src/browser.rs`: WebView の初期化、表示領域と DPI の同期、履歴操作、リーダー表示の適用
- `src/browser_runtime_test.rs`: ローカルページを使用した WebView の統合テスト
- `assets/icons/`: ツールバーなどのアイコン（SVG、実行ファイルに埋め込む）
- `assets/icon/`: アプリアイコン（元の `icon.svg` と、そこから作った Windows 用 `icon.ico`、macOS 用 `icon.icns`）
- `build.rs`: Windows の実行ファイルにアプリアイコンを埋め込む
- `scripts/make-icons.ps1`: `icon.svg` から `icon.ico`・`icon.icns`・確認用の `icon-256.png` を作り直す（Windows 専用。Microsoft Edge で描画する）
- `assets/reader.js`: 抽出した本文で表示を置き換えるスクリプトと、その CSS
- `assets/readability/`: Mozilla Readability.js 0.6.0 とライセンス（npm の `@mozilla/readability` から無改変で同梱）

GPUI は公開済みの `0.2.2` を使用しています。開発中の GitHub main とは API が異なります。

## ライセンス

MIT License です（`LICENSE`）。同梱している Readability.js は Apache License 2.0 です（`assets/readability/LICENSE.md`）。
