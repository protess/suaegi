//! ADF(Atlassian Document Format) → Markdown, **읽기 전용 순수 함수**(§2, mutation-verifiable
//! 코어). Jira Cloud v3의 description/comment 바디는 ADF JSON, Server/DC v2는 plain text라
//! 변환 불필요(호출부가 문자열 바디를 그대로 넘긴다).
//!
//! Orca `adf-markdown.ts`의 블록 구조를 포팅한다: doc/paragraph/heading/bulletList/orderedList/
//! listItem/codeBlock/blockquote/rule/hardBreak. **더해서** inline **마크**(bold/italic/inline
//! code/link)를 Markdown으로 렌더한다 — 이건 Orca를 **넘어서는 확장**이다(Orca의 `renderInline`은
//! 마크를 무시하고 raw text만 낸다). 브리프가 링크/볼드/코드 마크를 요구하므로 추가했다.
//!
//! # Deferred(무성 손실 아님 — 문서화)
//! - `table` / `panel` / `taskList` / `decisionList` 등 구조 블록: 내부 텍스트로 **평탄화**
//!   (구조는 잃되 텍스트는 보존, Orca 폴백과 동형). 표 레이아웃/체크박스 상태는 안 나온다.
//! - `media` / `mediaSingle` / `mediaGroup`: 텍스트가 없어 `[media]` 플레이스홀더.
//! - `mention` / `inlineCard` / `status` / `date` / `emoji`: `attrs.text|shortName|url` 폴백
//!   (예: `@name`, 카드 URL) — Orca와 동일.

use serde_json::Value;

/// 블록 종류. 리스트 항목의 연속 블록 들여쓰기 규칙(리스트 vs 비-리스트)에 쓰인다(Orca 미러).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    Block,
    List,
}

struct MarkdownBlock {
    kind: BlockKind,
    text: String,
}

/// **진입점.** ADF 값(또는 문자열/배열) → Markdown. 후처리(trailing 공백·과다 개행 정리·trim)는
/// Orca `adfToMarkdownText` 미러.
pub fn adf_to_markdown(value: &Value) -> String {
    let text = render_block(value).text;
    // `[ \t]+\n` → `\n` (줄 끝 공백 제거), `\n{3,}` → `\n\n` (과다 개행 축약), 그리고 trim.
    let text = strip_trailing_ws_before_newline(&text);
    let text = collapse_blank_lines(&text);
    text.trim().to_string()
}

fn strip_trailing_ws_before_newline(s: &str) -> String {
    s.split('\n')
        .map(|line| line.trim_end_matches([' ', '\t']))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 3개 이상 연속 개행 → 2개(`\n{3,}` → `\n\n`).
fn collapse_blank_lines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut run = 0usize;
    for ch in s.chars() {
        if ch == '\n' {
            run += 1;
            if run <= 2 {
                out.push(ch);
            }
        } else {
            run = 0;
            out.push(ch);
        }
    }
    out
}

fn is_integer_positive(v: &Value, fallback: i64) -> i64 {
    match v.as_i64() {
        Some(n) if n > 0 => n,
        _ => fallback,
    }
}

fn heading_level(v: &Value) -> usize {
    is_integer_positive(v, 1).clamp(1, 6) as usize
}

fn as_str(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

/// inline 렌더. text 노드는 **마크를 적용**한다(Orca를 넘는 확장). hardBreak → `\n`. 그 밖의
/// inline 노드는 `attrs.text|shortName|url` 폴백, 없으면 content 재귀(Orca 미러).
fn render_inline(node: &Value) -> String {
    match node {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().map(render_inline).collect(),
        Value::Object(_) => {
            if let Some(text) = node.get("text").and_then(Value::as_str) {
                return apply_marks(text, node.get("marks"));
            }
            if as_str(&node["type"]) == "hardBreak" {
                return "\n".to_string();
            }
            // 텍스트 없는 leaf 미디어는 플레이스홀더(무성 손실 아님).
            if as_str(&node["type"]) == "media" {
                return "[media]".to_string();
            }
            let attrs = node.get("attrs");
            let fallback = attrs
                .map(|a| {
                    let t = as_str(&a["text"]);
                    if !t.is_empty() {
                        return t.to_string();
                    }
                    let sn = as_str(&a["shortName"]);
                    if !sn.is_empty() {
                        return sn.to_string();
                    }
                    as_str(&a["url"]).to_string()
                })
                .unwrap_or_default();
            if !fallback.is_empty() {
                return fallback;
            }
            render_inline(&node["content"])
        }
        _ => String::new(),
    }
}

/// 텍스트에 ADF 마크를 Markdown으로 감싼다. **고정 순서**(마크 배열 순서와 무관하게 결정적):
/// code(가장 안쪽) → strong → em → link(가장 바깥). 예: strong+link → `[**t**](href)`.
fn apply_marks(text: &str, marks: Option<&Value>) -> String {
    let Some(Value::Array(marks)) = marks else {
        return text.to_string();
    };
    let mut has_code = false;
    let mut has_strong = false;
    let mut has_em = false;
    let mut href: Option<String> = None;
    for m in marks {
        match as_str(&m["type"]) {
            "code" => has_code = true,
            "strong" => has_strong = true,
            "em" => has_em = true,
            "link" => {
                let h = as_str(&m["attrs"]["href"]);
                if !h.is_empty() {
                    href = Some(h.to_string());
                }
            }
            _ => {}
        }
    }
    let mut s = text.to_string();
    if has_code {
        s = format!("`{s}`");
    }
    if has_strong {
        s = format!("**{s}**");
    }
    if has_em {
        s = format!("*{s}*");
    }
    if let Some(h) = href {
        s = format!("[{s}]({h})");
    }
    s
}

fn as_array(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}

fn join_blocks(blocks: &[MarkdownBlock]) -> String {
    blocks
        .iter()
        .map(|b| b.text.as_str())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn render_blocks(content: &Value) -> Vec<MarkdownBlock> {
    as_array(content)
        .iter()
        .map(render_block)
        .filter(|b| !b.text.is_empty())
        .collect()
}

fn render_list_item(node: &Value, prefix: &str) -> String {
    let blocks = render_blocks(&node["content"]);
    if blocks.is_empty() {
        return prefix.trim_end().to_string();
    }
    let mut lines: Vec<String> = Vec::new();
    let continuation = " ".repeat(prefix.chars().count());
    for (block_index, block) in blocks.iter().enumerate() {
        let block_lines: Vec<&str> = block.text.split('\n').collect();
        if block_index == 0 {
            lines.push(format!("{prefix}{}", block_lines.first().copied().unwrap_or("")).trim_end().to_string());
            for line in &block_lines[1..] {
                lines.push(format!("{continuation}{line}").trim_end().to_string());
            }
            continue;
        }
        if block.kind != BlockKind::List {
            lines.push(String::new());
        }
        for line in &block_lines {
            lines.push(format!("{continuation}{line}").trim_end().to_string());
        }
    }
    lines.join("\n")
}

fn render_list(record: &Value, ordered: bool) -> String {
    let start = if ordered {
        is_integer_positive(&record["attrs"]["order"], 1)
    } else {
        1
    };
    as_array(&record["content"])
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let prefix = if ordered {
                format!("{}. ", start + index as i64)
            } else {
                "- ".to_string()
            };
            render_list_item(item, &prefix)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_code_block(record: &Value) -> MarkdownBlock {
    let inner = render_inline(&record["content"]);
    let text = inner.strip_suffix('\n').unwrap_or(&inner);
    MarkdownBlock {
        kind: BlockKind::Block,
        text: format!("```\n{text}\n```"),
    }
}

fn render_blockquote(record: &Value) -> MarkdownBlock {
    let inner = join_blocks(&render_blocks(&record["content"]));
    let text = inner
        .split('\n')
        .map(|line| format!("> {line}").trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n");
    MarkdownBlock {
        kind: BlockKind::Block,
        text,
    }
}

fn block(text: String) -> MarkdownBlock {
    MarkdownBlock {
        kind: BlockKind::Block,
        text,
    }
}

fn render_block(node: &Value) -> MarkdownBlock {
    match node {
        Value::String(s) => return block(s.clone()),
        Value::Array(_) => return block(join_blocks(&render_blocks(node))),
        Value::Object(_) => {}
        _ => return block(String::new()),
    }
    match as_str(&node["type"]) {
        "doc" => block(join_blocks(&render_blocks(&node["content"]))),
        "paragraph" => block(render_inline(&node["content"])),
        "heading" => {
            let prefix = "#".repeat(heading_level(&node["attrs"]["level"]));
            block(format!("{prefix} {}", render_inline(&node["content"]).trim()).trim().to_string())
        }
        "bulletList" => MarkdownBlock {
            kind: BlockKind::List,
            text: render_list(node, false),
        },
        "orderedList" => MarkdownBlock {
            kind: BlockKind::List,
            text: render_list(node, true),
        },
        "listItem" => MarkdownBlock {
            kind: BlockKind::List,
            text: render_list_item(node, "- "),
        },
        "codeBlock" => render_code_block(node),
        "blockquote" => render_blockquote(node),
        "rule" => block("---".to_string()),
        // Deferred/unknown 블록 → 내부 텍스트로 평탄화(구조는 잃되 텍스트 보존, Orca 폴백 미러).
        _ => {
            let joined = join_blocks(&render_blocks(&node["content"]));
            if !joined.is_empty() {
                block(joined)
            } else {
                block(render_inline(node))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Server/DC 바디는 plain string → 그대로(변환은 호출부가 안 함). string 노드는 그대로.
    #[test]
    fn plain_string_passes_through() {
        assert_eq!(adf_to_markdown(&json!("hello world")), "hello world");
    }

    #[test]
    fn null_is_empty() {
        assert_eq!(adf_to_markdown(&Value::Null), "");
    }

    /// **mutation 코어 (d): 실제 ADF 픽스처 → 정확한 Markdown 문자열 고정.** nested list + code +
    /// link + bold를 담은 대표 doc. 어떤 노드 핸들러를 mutate해도(코드펜스 제거, 리스트 마커
    /// 변경, 마크 누락 등) 이 정확-문자열 비교가 깨진다. (Orca `issues.test.ts:344-443`의 실제
    /// PM-33 ADF를 베이스로 codeBlock/마크 문단을 덧댔다 — 발명한 trivial 픽스처 아님.)
    #[test]
    fn real_adf_doc_exact_markdown_output() {
        let doc = json!({
            "type": "doc",
            "version": 1,
            "content": [
                {
                    "type": "paragraph",
                    "content": [
                        { "type": "text", "text": "História" },
                        { "type": "hardBreak" },
                        { "type": "text", "text": "Coverage ownership" }
                    ]
                },
                {
                    "type": "bulletList",
                    "content": [
                        { "type": "listItem", "content": [
                            { "type": "paragraph", "content": [{ "type": "text", "text": "admin - JOAO" }] }
                        ]},
                        { "type": "listItem", "content": [
                            { "type": "paragraph", "content": [{ "type": "text", "text": "attachment batch - JOAO" }] }
                        ]}
                    ]
                },
                {
                    "type": "orderedList",
                    "content": [
                        { "type": "listItem", "content": [
                            { "type": "paragraph", "content": [{ "type": "text", "text": "API module" }] }
                        ]},
                        { "type": "listItem", "content": [
                            { "type": "paragraph", "content": [{ "type": "text", "text": "UI module" }] }
                        ]}
                    ]
                },
                {
                    "type": "codeBlock",
                    "attrs": { "language": "rust" },
                    "content": [{ "type": "text", "text": "let x = 1;\nlet y = 2;" }]
                },
                {
                    "type": "paragraph",
                    "content": [
                        { "type": "text", "text": "See " },
                        { "type": "text", "text": "the docs", "marks": [
                            { "type": "link", "attrs": { "href": "https://example.com" } }
                        ]},
                        { "type": "text", "text": " and run " },
                        { "type": "text", "text": "make", "marks": [{ "type": "code" }] },
                        { "type": "text", "text": " " },
                        { "type": "text", "text": "now", "marks": [{ "type": "strong" }] }
                    ]
                }
            ]
        });

        let expected = [
            "História",
            "Coverage ownership",
            "",
            "- admin - JOAO",
            "- attachment batch - JOAO",
            "",
            "1. API module",
            "2. UI module",
            "",
            "```",
            "let x = 1;",
            "let y = 2;",
            "```",
            "",
            "See [the docs](https://example.com) and run `make` **now**",
        ]
        .join("\n");

        assert_eq!(adf_to_markdown(&doc), expected);
    }

    /// heading + blockquote + rule, 그리고 italic 마크.
    #[test]
    fn heading_blockquote_rule_and_italic() {
        let doc = json!({
            "type": "doc",
            "content": [
                { "type": "heading", "attrs": { "level": 2 },
                  "content": [{ "type": "text", "text": "Summary" }] },
                { "type": "blockquote", "content": [
                    { "type": "paragraph", "content": [{ "type": "text", "text": "quoted line" }] }
                ]},
                { "type": "rule" },
                { "type": "paragraph", "content": [
                    { "type": "text", "text": "emph", "marks": [{ "type": "em" }] }
                ]}
            ]
        });
        let expected = ["## Summary", "", "> quoted line", "", "---", "", "*emph*"].join("\n");
        assert_eq!(adf_to_markdown(&doc), expected);
    }

    /// 마크가 겹치면 고정 순서로 중첩: strong+link → `[**t**](href)`.
    #[test]
    fn overlapping_marks_nest_deterministically() {
        let node = json!({ "type": "text", "text": "t", "marks": [
            { "type": "link", "attrs": { "href": "u" } },
            { "type": "strong" }
        ]});
        // 마크 배열 순서(link 먼저)와 무관하게 code<strong<em<link 순서로 감싼다.
        assert_eq!(apply_marks("t", node.get("marks")), "[**t**](u)");
    }

    /// deferred 노드(table)는 내부 텍스트로 평탄화(무성 손실 아님).
    #[test]
    fn deferred_table_flattens_to_inner_text() {
        let doc = json!({
            "type": "doc",
            "content": [{
                "type": "table",
                "content": [{
                    "type": "tableRow",
                    "content": [{
                        "type": "tableCell",
                        "content": [{ "type": "paragraph",
                            "content": [{ "type": "text", "text": "cell text" }] }]
                    }]
                }]
            }]
        });
        assert_eq!(adf_to_markdown(&doc), "cell text");
    }

    /// media leaf(텍스트 없음)는 플레이스홀더.
    #[test]
    fn media_leaf_is_placeholder() {
        let doc = json!({
            "type": "doc",
            "content": [{ "type": "paragraph", "content": [
                { "type": "media", "attrs": { "id": "abc", "type": "file" } }
            ]}]
        });
        assert_eq!(adf_to_markdown(&doc), "[media]");
    }

    /// mention은 attrs.text 폴백(Orca 미러).
    #[test]
    fn mention_falls_back_to_attrs_text() {
        let doc = json!({
            "type": "doc",
            "content": [{ "type": "paragraph", "content": [
                { "type": "text", "text": "cc " },
                { "type": "mention", "attrs": { "id": "1", "text": "@Ada" } }
            ]}]
        });
        assert_eq!(adf_to_markdown(&doc), "cc @Ada");
    }
}
