// 表示中のページを Readability.js で抽出した本文だけの表示に置き換える。
// browser.rs が Readability.js と同じ即時関数に入れて実行するため、ページのグローバルを汚さない。
if (document.documentElement.dataset.bReader === "on") return;

let article = null;
try {
  // 文字列にせず要素のまま受け取り、Trusted Types のページでも innerHTML を使わずに済ませる。
  article = new Readability(document.cloneNode(true), { serializer: (el) => el }).parse();
} catch {
  article = null;
}
const text = article && article.textContent ? article.textContent.trim() : "";
if (!article || !article.content || text.length < 200) {
  if (window.ipc) window.ipc.postMessage("reader-failed");
  return;
}

const el = (tag, className, content) => {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (content) node.textContent = content;
  return node;
};
const minutes = Math.max(1, Math.round(text.length / 500));
const main = el("main", "b-reader");
const meta = el("div", "b-reader-meta");
meta.append(
  el("span", "b-reader-site", article.siteName || location.hostname.replace(/^www\./, "")),
  el("span", "", `約 ${minutes} 分で読めます`),
);
main.append(meta, el("h1", "", article.title || document.title));
if (article.byline) main.append(el("p", "b-reader-byline", article.byline));
const content = el("div", "b-reader-content");
content.append(document.importNode(article.content, true));
main.append(content);
const body = document.createElement("body");
body.append(main);

// <style> の挿入はページの CSP で止められることがあるため、CSSOM で当てる。
const sheet = new CSSStyleSheet();
sheet.replaceSync(`
html { background: #ffffff; color-scheme: light; }
body { margin: 0; background: #ffffff; color: #1d2733; font-family: 'Segoe UI', 'Yu Gothic UI', 'Hiragino Sans', sans-serif; font-size: 17px; line-height: 1.9; letter-spacing: 0.02em; overflow-wrap: anywhere; }
.b-reader { max-width: 38em; margin: 0 auto; padding: 48px 40px 80px; }
.b-reader-meta { display: flex; flex-wrap: wrap; gap: 4px 12px; font-size: 13px; letter-spacing: 0; color: #5b6675; }
.b-reader-site { font-weight: 600; color: #3d4856; }
.b-reader h1 { margin: 12px 0 0; font-size: 1.7em; line-height: 1.45; letter-spacing: 0.01em; }
.b-reader-byline { margin: 8px 0 0; font-size: 14px; color: #5b6675; }
.b-reader-content { margin-top: 24px; }
.b-reader-content h1, .b-reader-content h2 { margin: 40px 0 0; font-size: 1.25em; line-height: 1.5; }
.b-reader-content h3, .b-reader-content h4, .b-reader-content h5, .b-reader-content h6 { margin: 32px 0 0; font-size: 1.1em; line-height: 1.5; }
.b-reader-content p, .b-reader-content ul, .b-reader-content ol, .b-reader-content blockquote, .b-reader-content pre, .b-reader-content table { margin: 16px 0 0; }
.b-reader-content figure { margin: 24px 0 0; }
.b-reader-content figcaption { margin-top: 6px; font-size: 13px; color: #5b6675; }
.b-reader-content img, .b-reader-content video, .b-reader-content svg { max-width: 100%; height: auto; }
.b-reader-content a { color: #0a64a4; }
.b-reader-content blockquote { margin-left: 0; padding-left: 16px; border-left: 3px solid #d5dbe2; color: #46515f; }
.b-reader-content pre { padding: 14px 16px; overflow-x: auto; background: #f6f7f9; border: 1px solid #e1e5ea; border-radius: 8px; font-size: 0.82em; line-height: 1.6; letter-spacing: 0; }
.b-reader-content code { font-family: Consolas, 'SF Mono', Menlo, monospace; }
.b-reader-content :not(pre) > code { padding: 1px 4px; border-radius: 4px; background: #f0f2f5; font-size: 0.9em; }
.b-reader-content table { display: block; overflow-x: auto; border-collapse: collapse; font-size: 0.9em; line-height: 1.6; }
.b-reader-content th, .b-reader-content td { padding: 8px 12px; border-bottom: 1px solid #e1e5ea; text-align: left; vertical-align: top; }
.b-reader-content th { border-bottom: 2px solid #d5dbe2; }
.b-reader-content hr { margin: 32px 0; border: 0; border-top: 1px solid #e1e5ea; }
`);
document.querySelectorAll('link[rel~="stylesheet"], style').forEach((node) => node.remove());
document.documentElement.removeAttribute("style");
document.adoptedStyleSheets = [sheet];
document.body.replaceWith(body);
document.documentElement.dataset.bReader = "on";
window.scrollTo(0, 0);
