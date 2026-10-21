//! 写信用的 HTML 处理：
//! - `clean`：粘贴进编辑器的 HTML（Word、网页）按白名单清理，只留常见排版；
//! - `extract_data_images`：编辑器里插入的图片是 data: URL，发信前换成 cid 内嵌图片。

use base64::Engine;
use dom_query::{Document, NodeRef};

/// 整个元素连同内容删除
const DROP: &[&str] = &[
    "script", "style", "iframe", "frame", "frameset", "object", "embed", "applet", "form", "input",
    "button", "select", "textarea", "option", "meta", "link", "base", "title", "head", "svg",
    "math", "noscript", "template", "xml", "video", "audio", "canvas",
];

/// 保留的元素；其他元素去掉标签、保留内容（如 Word 的 o:p、section）
const KEEP: &[&str] = &[
    "p",
    "div",
    "span",
    "br",
    "b",
    "strong",
    "i",
    "em",
    "u",
    "s",
    "strike",
    "del",
    "ins",
    "sub",
    "sup",
    "small",
    "big",
    "font",
    "ul",
    "ol",
    "li",
    "a",
    "img",
    "table",
    "thead",
    "tbody",
    "tfoot",
    "tr",
    "td",
    "th",
    "caption",
    "colgroup",
    "col",
    "blockquote",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "pre",
    "code",
];

const STYLE_PROPS: &[&str] = &[
    "color",
    "background-color",
    "font-size",
    "font-family",
    "font-weight",
    "font-style",
    "text-decoration",
    "text-align",
    "text-indent",
    "vertical-align",
    "line-height",
    "white-space",
    "width",
    "height",
    "max-width",
    "border",
    "border-top",
    "border-right",
    "border-bottom",
    "border-left",
    "border-color",
    "border-style",
    "border-width",
    "border-collapse",
    "border-spacing",
    "padding",
    "padding-top",
    "padding-right",
    "padding-bottom",
    "padding-left",
    "margin",
    "margin-top",
    "margin-right",
    "margin-bottom",
    "margin-left",
    "list-style-type",
];

/// 嵌套太深的内容直接丢弃，避免递归过深
const MAX_DEPTH: usize = 200;

fn attrs_for(tag: &str) -> &'static [&'static str] {
    match tag {
        "a" => &["href", "style"],
        "img" => &["src", "alt", "width", "height", "style"],
        "td" | "th" => &["colspan", "rowspan", "align", "valign", "width", "style"],
        "table" => &[
            "border",
            "cellpadding",
            "cellspacing",
            "width",
            "align",
            "style",
        ],
        "col" | "colgroup" => &["span", "width", "style"],
        "font" => &["color", "face", "size", "style"],
        "ol" => &["start", "type", "style"],
        "p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => &["align", "style"],
        _ => &["style"],
    }
}

fn safe_href(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    !lower.chars().any(|c| c.is_control())
        && ["http://", "https://", "mailto:", "tel:"]
            .iter()
            .any(|scheme| lower.starts_with(scheme))
}

fn safe_src(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    !lower.chars().any(|c| c.is_control())
        && (lower.starts_with("https://")
            || lower.starts_with("http://")
            || lower.starts_with("cid:")
            || (lower.starts_with("data:image/")
                && lower.contains(";base64,")
                && !lower.starts_with("data:image/svg")))
}

/// 只保留白名单里的 CSS 属性，丢掉 mso-*、url()、expression 等。
fn clean_style(style: &str) -> String {
    style
        .split(';')
        .filter_map(|decl| {
            let (prop, value) = decl.split_once(':')?;
            let prop = prop.trim().to_ascii_lowercase();
            let value = value.trim();
            let lower = value.to_ascii_lowercase();
            let unsafe_value = [
                "url(",
                "expression",
                "javascript",
                "@import",
                "\\",
                "<",
                ">",
            ]
            .iter()
            .any(|bad| lower.contains(bad));
            (STYLE_PROPS.contains(&prop.as_str()) && !value.is_empty() && !unsafe_value)
                .then(|| format!("{prop}:{value}"))
        })
        .collect::<Vec<_>>()
        .join(";")
}

fn clean_attrs(node: &NodeRef, tag: &str) {
    let allowed = attrs_for(tag);
    for attr in node.attrs() {
        let name = attr.name.local.to_string().to_ascii_lowercase();
        let value = attr.value.to_string();
        let keep = allowed.contains(&name.as_str())
            && match name.as_str() {
                "href" => safe_href(&value),
                "src" => safe_src(&value),
                _ => true,
            };
        if !keep {
            node.remove_attr(&attr.name.local);
        } else if name == "style" {
            let style = clean_style(&value);
            if style.is_empty() {
                node.remove_attr("style");
            } else {
                node.set_attr("style", &style);
            }
        }
    }
}

fn walk(node: &NodeRef, depth: usize, block_remote: bool) {
    for child in node.children() {
        if child.is_comment() || child.is_doctype() {
            child.remove_from_parent();
            continue;
        }
        if !child.is_element() {
            continue;
        }
        if depth >= MAX_DEPTH {
            child.remove_from_parent();
            continue;
        }
        let tag = child
            .node_name()
            .map(|n| n.to_ascii_lowercase())
            .unwrap_or_default();
        if DROP.contains(&tag.as_str()) {
            child.remove_from_parent();
            continue;
        }
        // 远程图片会在打开时向外发请求（追踪像素），重新打开草稿时不加载
        if block_remote
            && tag == "img"
            && child
                .attr("src")
                .is_some_and(|src| src.trim().to_ascii_lowercase().starts_with("http"))
        {
            child.remove_from_parent();
            continue;
        }
        walk(&child, depth + 1, block_remote);
        if KEEP.contains(&tag.as_str()) {
            clean_attrs(&child, &tag);
        } else {
            // 去掉标签、保留内容（dom_query 的 unwrap_node 去掉的是父元素，不能用）
            for grandchild in child.children() {
                child.insert_before(&grandchild);
            }
            child.remove_from_parent();
        }
    }
}

fn clean_with(html: &str, block_remote: bool) -> String {
    // 片段文档的内容挂在一个 <html> 元素下面
    let doc = Document::fragment(html);
    let root = doc.html_root();
    walk(&root, 0, block_remote);
    root.inner_html().to_string()
}

/// 清理粘贴进编辑器的 HTML 片段。
pub fn clean(html: &str) -> String {
    clean_with(html, false)
}

/// 清理并去掉远程图片：用于把已有邮件（草稿）放回编辑器，避免打开时加载追踪像素。
pub fn clean_blocking_remote(html: &str) -> String {
    clean_with(html, true)
}

/// 插入正文的单张图片上限（太大的图应作为附件发送）
pub const MAX_INLINE_IMAGE_BYTES: usize = 10 * 1024 * 1024;

/// 把选中的本地图片转成编辑器能直接显示的 data URL。只接受常见位图格式。
pub fn image_data_url(name: &str, data: &[u8]) -> anyhow::Result<String> {
    let content_type = crate::smtp::content_type_for(name);
    if !content_type.starts_with("image/") || content_type.contains("svg") {
        anyhow::bail!("只能插入 PNG、JPEG、GIF、WebP 等图片");
    }
    if data.len() > MAX_INLINE_IMAGE_BYTES {
        anyhow::bail!(
            "图片超过 {} MB，请作为附件发送",
            MAX_INLINE_IMAGE_BYTES / 1024 / 1024
        );
    }
    Ok(format!(
        "data:{content_type};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(data)
    ))
}

/// 编辑器里的 data: 图片，发信时作为内嵌部件附上。
pub struct InlineImage {
    pub cid: String,
    pub content_type: String,
    pub data: Vec<u8>,
}

pub(crate) fn decode_data_url(url: &str) -> Option<(String, Vec<u8>)> {
    let rest = url.trim().strip_prefix("data:").or_else(|| {
        url.trim()
            .get(..5)
            .filter(|p| p.eq_ignore_ascii_case("data:"))
            .map(|_| &url.trim()[5..])
    })?;
    let (meta, payload) = rest.split_once(',')?;
    let meta = meta.to_ascii_lowercase();
    let mut params = meta.split(';').map(str::trim);
    // 只保留 type/subtype，charset、name 之类的参数不带进邮件头
    let content_type = params.next()?.to_string();
    if !params.any(|p| p == "base64") {
        return None;
    }
    if !content_type.starts_with("image/")
        || content_type.contains("svg")
        || !content_type[6..]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
    {
        return None;
    }
    let cleaned: String = payload.chars().filter(|c| !c.is_whitespace()).collect();
    let data = base64::engine::general_purpose::STANDARD
        .decode(cleaned)
        .ok()?;
    Some((content_type, data))
}

/// 标签结束位置（`>` 之后）；引号里的 `>` 不算。
fn tag_end(html: &str, start: usize) -> Option<usize> {
    let mut quote = None;
    for (i, c) in html[start..].char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '>') => return Some(start + i + 1),
            _ => {}
        }
    }
    None
}

/// 把 `<img src="data:image/...">` 换成 `cid:` 引用，返回新 HTML 和要附上的图片。
/// 只替换图片地址本身，其余内容逐字保留（不重新序列化，以免改动引用原文的结构）。
/// 编辑器的 HTML 由浏览器序列化，属性值总是用双引号；无效的 data 图片整个去掉。
pub fn extract_data_images(html: &str) -> (String, Vec<InlineImage>) {
    let lower = html.to_ascii_lowercase();
    if !lower.contains("data:") {
        return (html.to_string(), Vec::new());
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let mut images: Vec<InlineImage> = Vec::new();
    let mut by_url: std::collections::HashMap<&str, String> = std::collections::HashMap::new();
    let mut out = String::with_capacity(html.len());
    let mut copied = 0;
    let mut search = 0;
    while let Some(found) = lower[search..].find("<img") {
        let tag_start = search + found;
        let Some(tag_end) = tag_end(&lower, tag_start) else {
            break;
        };
        search = tag_end;
        let tag = &lower[tag_start..tag_end];
        let Some(attr) = tag.find(" src=\"data:") else {
            continue;
        };
        let value_start = tag_start + attr + " src=\"".len();
        let Some(value_len) = html[value_start..tag_end].find('"') else {
            continue;
        };
        let value_end = value_start + value_len;
        let url = &html[value_start..value_end];
        let cid = match by_url.get(url) {
            Some(cid) => Some(cid.clone()),
            None => decode_data_url(url).map(|(content_type, data)| {
                let cid = format!("img{}.{nanos:x}@mailbeacon", images.len() + 1);
                by_url.insert(url, cid.clone());
                images.push(InlineImage {
                    cid: cid.clone(),
                    content_type,
                    data,
                });
                cid
            }),
        };
        match cid {
            Some(cid) => {
                out.push_str(&html[copied..value_start]);
                out.push_str("cid:");
                out.push_str(&cid);
                copied = value_end;
            }
            None => {
                out.push_str(&html[copied..tag_start]);
                copied = tag_end;
            }
        }
    }
    out.push_str(&html[copied..]);
    (out, images)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_removes_scripts_handlers_and_unsafe_urls() {
        let out = clean(
            r#"<p onclick="evil()">你好<script>alert(1)</script><style>p{color:red}</style></p>
               <a href="javascript:alert(1)">坏链接</a><a href="https://example.com" target="_blank">好链接</a>
               <a href="mailto:person@example.com">邮件</a><a href="tel:+8613812345678">电话</a>
               <img src="https://example.com/x.png" onerror="evil()"><iframe src="https://x"></iframe>
               <form><input value="x"></form>"#,
        );
        assert!(out.contains("你好"));
        for bad in [
            "script",
            "alert",
            "onclick",
            "onerror",
            "<style",
            "iframe",
            "<form",
            "<input",
            "javascript:",
        ] {
            assert!(!out.contains(bad), "{bad} 应被清除：{out}");
        }
        assert!(out.contains(r#"href="https://example.com""#));
        assert!(out.contains(r#"href="mailto:person@example.com""#));
        assert!(out.contains(r#"href="tel:+8613812345678""#));
        assert!(out.contains("坏链接"), "去掉危险链接但保留文字");
        assert!(out.contains(r#"src="https://example.com/x.png""#));
    }

    #[test]
    fn clean_keeps_formatting_tables_and_safe_styles() {
        let out = clean(
            r#"<div style="text-align:center;color:#c00;position:fixed;background:url(https://x/t.png)">
               <b>粗</b><i>斜</i><u>下划线</u><span style="font-size:18px;font-family:'宋体'">字</span>
               <ul><li>一</li></ul><table border="1"><tr><td colspan="2">格</td></tr></table></div>"#,
        );
        for good in [
            "<b>粗</b>",
            "<i>斜</i>",
            "<u>下划线</u>",
            "<ul>",
            "<li>一</li>",
            "<table",
            r#"colspan="2""#,
            "text-align:center",
            "color:#c00",
            "font-size:18px",
        ] {
            assert!(out.contains(good), "{good} 应保留：{out}");
        }
        assert!(!out.contains("position"), "{out}");
        assert!(!out.contains("url("), "{out}");
    }

    #[test]
    fn clean_unwraps_word_markup_and_drops_comments() {
        let out = clean(
            r#"<!--[if gte mso 9]><xml><o:OfficeDocumentSettings/></xml><![endif]-->
               <p class="MsoNormal" style="mso-margin-top-alt:auto;margin:0"><o:p>段落</o:p></p>"#,
        );
        assert!(out.contains("段落"));
        assert!(
            !out.contains("o:p")
                && !out.contains("mso-")
                && !out.contains("MsoNormal")
                && !out.contains("<!--"),
            "{out}"
        );
    }

    #[test]
    fn reopened_drafts_can_drop_remote_images() {
        let html = r#"<p>正文<img src="https://tracker.example/p.gif"><img src="http://x/y.png"><img src="data:image/png;base64,iVBORw=="><img src="cid:a@b"></p>"#;
        let kept = clean(html);
        assert!(kept.contains("tracker.example"), "粘贴时保留远程图片");
        let blocked = clean_blocking_remote(html);
        assert!(
            !blocked.contains("tracker.example") && !blocked.contains("http://x"),
            "{blocked}"
        );
        assert!(
            blocked.contains("data:image/png")
                && blocked.contains("cid:a@b")
                && blocked.contains("正文")
        );
    }

    #[test]
    fn data_urls_with_uppercase_or_parameters_are_still_embedded() {
        let html = r#"<p><img src="data:image/PNG;BASE64,iVBORw=="><img src="data:image/jpeg;name=a.jpg;charset=utf-8;base64,/9g="></p>"#;
        let (out, images) = extract_data_images(html);
        assert_eq!(images.len(), 2, "{out}");
        assert_eq!(images[0].content_type, "image/png");
        assert_eq!(images[1].content_type, "image/jpeg");
        assert!(!out.contains("data:image"));
    }

    #[test]
    fn extracting_images_does_not_reserialize_the_rest_of_the_document() {
        let html = "<!doctype html><html><head><style>.q p{margin:0}</style></head><body><p><div>x</div></p><img src=\"data:image/png;base64,iVBORw==\"></body></html>";
        let (out, images) = extract_data_images(html);
        assert_eq!(images.len(), 1);
        let expected = html.replace(
            "data:image/png;base64,iVBORw==",
            &format!("cid:{}", images[0].cid),
        );
        assert_eq!(out, expected, "除了图片地址，其余内容逐字不变");
    }

    #[test]
    fn a_raw_angle_bracket_in_an_earlier_attribute_does_not_break_the_tag_scan() {
        let html = r#"<p><img alt="a > b" src="data:image/png;base64,iVBORw=="><img alt="c > d" src="data:image/png;base64,!!!">尾</p>"#;
        let (out, images) = extract_data_images(html);
        assert_eq!(images.len(), 1);
        assert!(
            out.contains(&format!(r#"alt="a > b" src="cid:{}""#, images[0].cid)),
            "{out}"
        );
        assert!(
            !out.contains("data:") && !out.contains("d\">"),
            "无效图片整个去掉，不留残片：{out}"
        );
        assert!(out.ends_with("尾</p>"));
    }

    #[test]
    fn picked_images_become_data_urls_only_for_raster_images_within_limit() {
        let url = image_data_url("照片.PNG", b"\x89PNG").unwrap();
        assert_eq!(url, "data:image/png;base64,iVBORw==");
        assert!(
            image_data_url("logo.svg", b"<svg/>").is_err(),
            "SVG 可以带脚本，不允许"
        );
        assert!(image_data_url("a.pdf", b"%PDF").is_err());
        assert!(image_data_url("big.jpg", &vec![0u8; MAX_INLINE_IMAGE_BYTES + 1]).is_err());
    }

    #[test]
    fn data_images_become_cid_parts_and_duplicates_share_one() {
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aB9sAAAAASUVORK5CYII=";
        let html = format!(
            r#"<p>图<img src="data:image/png;base64,{png}" alt="a"><img src="data:image/png;base64,{png}"></p><img src="cid:keep@x"><img src="data:image/png;base64,!!!">"#
        );
        let (out, images) = extract_data_images(&html);
        assert_eq!(images.len(), 1, "相同图片只附一份");
        assert_eq!(images[0].content_type, "image/png");
        assert!(images[0].data.starts_with(b"\x89PNG"));
        assert_eq!(out.matches(&format!("cid:{}", images[0].cid)).count(), 2);
        assert!(out.contains("cid:keep@x"), "已有的 cid 引用保持不变");
        assert!(
            !out.contains("data:image"),
            "无效的 data URL 也不能原样发出：{out}"
        );
    }
}
