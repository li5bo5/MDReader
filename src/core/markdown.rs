//! Markdown GFM 流式解析器，将源码转换为面向 Direct2D / DirectWrite 渲染的扁平块模型。
//!
//! 特性与安全保证：
//! - 使用 `pulldown-cmark`（`default-features = false`）并启用 `ENABLE_TABLES | ENABLE_STRIKETHROUGH | ENABLE_TASKLISTS`；
//! - 借助 `TextMergeStream` 合并碎裂文本事件，并在同样式下合并相邻 `RichSpan`；
//! - 采用纯堆分配迭代帧栈，面对 500+ 层超深嵌套引用或列表绝不栈溢出；
//! - 原始 HTML 仅做零脚本执行的安全纯文本降级。

use std::mem;

use pulldown_cmark::{
    Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd,
};

use crate::core::highlight::highlight_code;

/// 富文本片段列表类型别名（与 `Vec<RichSpan>` 完全等价）。
pub type RichText = Vec<RichSpan>;

/// GFM 表格列对齐方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableAlign {
    None,
    Left,
    Center,
    Right,
}

/// 代码词法高亮 Token 分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Plain,
    Keyword,
    String,
    Comment,
    Number,
    TypeOrFn,
}

/// 最小可排版内联富文本片段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RichSpan {
    /// 片段文本内容。
    pub text: String,
    /// 粗体样式。
    pub bold: bool,
    /// 斜体样式。
    pub italic: bool,
    /// 删除线样式。
    pub strike: bool,
    /// 等宽代码字体/背景标记（行内代码或代码块）。
    pub code: bool,
    /// 所属超链接目标 URL。
    pub link_url: Option<String>,
    /// 代码块内词法高亮 Token 类型（普通正文为 `None`）。
    pub highlight: Option<TokenKind>,
}

/// 顶层文档语义块类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockKind {
    /// 标题块（`level` 取值 `1..=6`）。
    Heading { level: u8 },
    /// 普通文本段落。
    Paragraph,
    /// 围栏或缩进代码块。
    CodeBlock { language: Option<String> },
    /// 引用块段落。
    BlockQuote,
    /// 有序、无序或任务列表项。
    ListItem {
        ordered: bool,
        index: Option<u64>,
        depth: u16,
        task: Option<bool>,
    },
    /// GFM 表格（`rows[0]` 为表头，`rows[1..]` 为表体）。
    Table {
        alignments: Vec<TableAlign>,
        rows: Vec<Vec<RichText>>,
    },
    /// 水平分割线。
    ThematicBreak,
    /// 独立块级图片。
    Image { url: String, alt: String },
}

/// 文档单个语义块及其内联富文本片段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocBlock {
    pub kind: BlockKind,
    pub spans: Vec<RichSpan>,
    /// 引用块嵌套深度（普通非引用块为 `0`，一层引用为 `1`，二层嵌套引用为 `2`，以此类推）。
    pub quote_depth: u16,
    /// 该块在源码中的 0-based 起始行号。
    pub start_line: usize,
    /// 该块在源码中的 0-based 结束行号（包含）。
    pub end_line: usize,
}

/// 侧边栏大纲目录条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineItem {
    /// 标题级别（`1..=6`）。
    pub level: u8,
    /// 展平后的标题纯文本。
    pub title: String,
    /// 对应 `ParsedDocument::blocks` 中的索引。
    pub block_index: usize,
    /// 对应源码 0-based 起始行号。
    pub start_line: usize,
}

/// 解析后的完整文档块序列。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedDocument {
    pub blocks: Vec<DocBlock>,
    /// 源码总行数（至少为 1，空串为 1）。
    pub total_lines: usize,
}

impl ParsedDocument {
    /// 提取文档中所有 `H1`~`H6` 标题构成的大纲列表。
    pub fn outline(&self) -> Vec<OutlineItem> {
        let mut items = Vec::new();
        for (idx, block) in self.blocks.iter().enumerate() {
            if let BlockKind::Heading { level } = block.kind {
                let mut raw = String::new();
                for span in &block.spans {
                    raw.push_str(&span.text);
                }
                let title = raw
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                if !title.is_empty() {
                    items.push(OutlineItem {
                        level,
                        title,
                        block_index: idx,
                        start_line: block.start_line,
                    });
                }
            }
        }
        items
    }
}

/// 源码字节偏移到 0-based 行号的快速索引表。
struct LineIndex {
    line_starts: Vec<usize>,
}

impl LineIndex {
    fn new(input: &str) -> Self {
        let bytes = input.as_bytes();
        let mut line_starts = Vec::with_capacity(bytes.len() / 40 + 2);
        line_starts.push(0);
        for (i, &b) in bytes.iter().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        Self { line_starts }
    }

    fn total_lines(&self) -> usize {
        self.line_starts.len().max(1)
    }

    fn line_of(&self, byte_offset: usize) -> usize {
        self.line_starts
            .partition_point(|&start| start <= byte_offset)
            .saturating_sub(1)
    }

    fn line_range(&self, range: &std::ops::Range<usize>) -> (usize, usize) {
        let start = self.line_of(range.start);
        let end_byte = if range.end > range.start {
            // 若块末尾停在换行符后一字节，退回一字节以获得实际内容所在行
            range.end - 1
        } else {
            range.start
        };
        let end = self.line_of(end_byte).max(start);
        (start, end)
    }
}

/// 当前活跃内联样式嵌套计数器。
#[derive(Debug, Clone, Default)]
struct InlineStyle {
    bold_depth: usize,
    italic_depth: usize,
    strike_depth: usize,
    link_stack: Vec<String>,
}

impl InlineStyle {
    /// 将新片段按当前样式推入缓冲，若样式与上一个非高亮片段完全一致则直接合并文本。
    fn push_span(&self, spans: &mut Vec<RichSpan>, text: &str, code: bool) {
        if text.is_empty() {
            return;
        }
        let bold = self.bold_depth > 0;
        let italic = self.italic_depth > 0;
        let strike = self.strike_depth > 0;
        let link_ref = self.link_stack.last().map(String::as_str);

        if let Some(last) = spans.last_mut()
            && last.bold == bold
            && last.italic == italic
            && last.strike == strike
            && last.code == code
            && last.link_url.as_deref() == link_ref
            && last.highlight.is_none()
        {
            last.text.push_str(text);
            return;
        }

        spans.push(RichSpan {
            text: text.to_string(),
            bold,
            italic,
            strike,
            code,
            link_url: link_ref.map(str::to_string),
            highlight: None,
        });
    }
}

struct ListState {
    ordered: bool,
    next_index: u64,
}

struct ListItemFrame {
    ordered: bool,
    index: Option<u64>,
    depth: u16,
    task: Option<bool>,
    spans: RichText,
    emitted: bool,
    in_first_para: bool,
    first_para_done: bool,
    start_line: usize,
    end_line: usize,
}

struct TableFrame {
    alignments: Vec<TableAlign>,
    rows: Vec<Vec<RichText>>,
    current_row: Option<Vec<RichText>>,
    current_cell: Option<RichText>,
    start_line: usize,
    end_line: usize,
}

struct ImageCapture {
    url: String,
    alt: String,
    nested_depth: usize,
    start_line: usize,
}

enum ActiveFrame {
    Heading {
        level: u8,
        spans: RichText,
        start_line: usize,
    },
    Paragraph {
        spans: RichText,
        quote_depth: u16,
        start_line: usize,
    },
    CodeBlock {
        language: Option<String>,
        buf: String,
        start_line: usize,
    },
    HtmlBlock {
        buf: String,
        start_line: usize,
    },
    ListItem(ListItemFrame),
    Table(TableFrame),
}

enum InlineHtmlAction {
    Newline,
    Ignore,
    PlainText,
}

/// 将 `pulldown-cmark` 标题级别映射为 `1..=6`。
fn heading_level_to_u8(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// 将 `pulldown-cmark` 对齐枚举映射为内部 `TableAlign`。
fn map_table_align(align: Alignment) -> TableAlign {
    match align {
        Alignment::None => TableAlign::None,
        Alignment::Left => TableAlign::Left,
        Alignment::Center => TableAlign::Center,
        Alignment::Right => TableAlign::Right,
    }
}

/// 若栈顶是尚未输出的 `ListItem`，立即将其输出到 `blocks` 并标记为已输出。
fn flush_pending_list_item(
    frames: &mut [ActiveFrame],
    blocks: &mut Vec<DocBlock>,
    fallback_end_line: usize,
) {
    if let Some(ActiveFrame::ListItem(item)) = frames.last_mut()
        && !item.emitted
    {
        blocks.push(DocBlock {
            kind: BlockKind::ListItem {
                ordered: item.ordered,
                index: item.index,
                depth: item.depth,
                task: item.task,
            },
            spans: mem::take(&mut item.spans),
            quote_depth: 0,
            start_line: item.start_line,
            end_line: fallback_end_line.max(item.start_line),
        });
        item.emitted = true;
    }
}

/// 获取当前内联富文本事件的目标 `Vec<RichSpan>` 缓冲。
fn active_spans_mut(frames: &mut [ActiveFrame]) -> Option<&mut Vec<RichSpan>> {
    match frames.last_mut()? {
        ActiveFrame::Heading { spans, .. } => Some(spans),
        ActiveFrame::Paragraph { spans, .. } => Some(spans),
        ActiveFrame::ListItem(item) => Some(&mut item.spans),
        ActiveFrame::Table(table) => table.current_cell.as_mut(),
        ActiveFrame::CodeBlock { .. } | ActiveFrame::HtmlBlock { .. } => None,
    }
}

/// 剥离代码块末尾由 `pulldown-cmark` 附带的单个换行符（保留内部空行）。
fn strip_trailing_single_newline(code: &str) -> &str {
    if let Some(stripped) = code.strip_suffix("\r\n") {
        stripped
    } else if let Some(stripped) = code.strip_suffix('\n') {
        stripped
    } else {
        code
    }
}

/// 判断内联 HTML 标签名称是否属于纯排版安全标签（不含 `script`/`style` 等危险标签）。
fn is_safe_inline_container_tag(tag_name: &str) -> bool {
    matches!(
        tag_name,
        "p" | "div"
            | "span"
            | "b"
            | "strong"
            | "i"
            | "em"
            | "u"
            | "s"
            | "del"
            | "ins"
            | "sub"
            | "sup"
            | "kbd"
            | "mark"
            | "small"
            | "code"
            | "a"
            | "details"
            | "summary"
    )
}

/// 提取形如 `<tag ...>` 或 `</tag>` 的规范化小写标签名（精确边界匹配）。
fn extract_html_tag_name(trimmed: &str) -> Option<String> {
    if !trimmed.starts_with('<') || !trimmed.ends_with('>') {
        return None;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    let after_slash = inner.strip_prefix('/').unwrap_or(inner).trim_start();
    let name: String = after_slash
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if name.is_empty() {
        return None;
    }
    let rest = &after_slash[name.len()..];
    if rest.is_empty()
        || rest.starts_with(char::is_whitespace)
        || rest.starts_with('/')
    {
        Some(name.to_ascii_lowercase())
    } else {
        None
    }
}

/// 对内联 HTML 片段分类处理，保证无脚本执行与原文安全呈现。
fn classify_inline_html(raw: &str) -> InlineHtmlAction {
    let trimmed = raw.trim();
    if trimmed.starts_with("<!--") && trimmed.ends_with("-->") {
        return InlineHtmlAction::Ignore;
    }

    if let Some(tag_name) = extract_html_tag_name(trimmed) {
        if tag_name == "br" {
            return InlineHtmlAction::Newline;
        }
        if is_safe_inline_container_tag(&tag_name) {
            return InlineHtmlAction::Ignore;
        }
    }

    InlineHtmlAction::PlainText
}

/// 将块级 HTML 安全降级为纯文本字符串；返回 `None` 表示应跳过（如纯注释或空白）。
fn sanitize_html_block_to_plain_text(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with("<!--") && trimmed.ends_with("-->") {
        return None;
    }

    let lower = trimmed.to_ascii_lowercase();
    if lower.contains("<script") || lower.contains("<style") {
        return Some(trimmed.to_string());
    }

    // 仅剥离尖括号包裹的无害 HTML 标签外壳，并将 `<br...>` 转为 `\n`
    let mut out = String::with_capacity(trimmed.len());
    let mut chars = trimmed.char_indices().peekable();
    while let Some((idx, ch)) = chars.next() {
        if ch == '<'
            && let Some(rel_end) = trimmed[idx..].find('>')
        {
            let tag_slice = &trimmed[idx..idx + rel_end + 1];
            if tag_slice.starts_with("<!--") && tag_slice.ends_with("-->") {
                while let Some(&(next_idx, _)) = chars.peek() {
                    if next_idx <= idx + rel_end {
                        chars.next();
                    } else {
                        break;
                    }
                }
                continue;
            }
            if let Some(name) = extract_html_tag_name(tag_slice) {
                if name == "br" {
                    out.push('\n');
                }
                while let Some(&(next_idx, _)) = chars.peek() {
                    if next_idx <= idx + rel_end {
                        chars.next();
                    } else {
                        break;
                    }
                }
                continue;
            }
        }
        out.push(ch);
    }

    let cleaned = out.trim();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned.to_string())
    }
}

/// 解析 Markdown 文本为供阅读器渲染层消费的 `ParsedDocument`。
pub fn parse_markdown(input: &str) -> ParsedDocument {
    let line_index = LineIndex::new(input);
    let total_lines = line_index.total_lines();

    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let stream = Parser::new_ext(input, options).into_offset_iter();

    let mut blocks: Vec<DocBlock> = Vec::new();
    let mut frames: Vec<ActiveFrame> = Vec::new();
    let mut list_stack: Vec<ListState> = Vec::new();
    let mut quote_depth: usize = 0;
    let mut style = InlineStyle::default();
    let mut image_capture: Option<ImageCapture> = None;

    for (event, range) in stream {
        let (ev_start_line, ev_end_line) = line_index.line_range(&range);

        // 1. 图片捕获态优先拦截，完整聚合 alt 文本后再按所在块上下文输出
        if let Some(cap) = image_capture.as_mut() {
            match event {
                Event::Start(Tag::Image { .. }) => {
                    cap.nested_depth += 1;
                    continue;
                }
                Event::End(TagEnd::Image) => {
                    cap.nested_depth = cap.nested_depth.saturating_sub(1);
                    if cap.nested_depth > 0 {
                        continue;
                    }
                    let finished = image_capture.take().expect("图片捕获态必定存在");
                    let inline_only = matches!(
                        frames.last(),
                        Some(ActiveFrame::Table(_) | ActiveFrame::Heading { .. })
                    ) || !style.link_stack.is_empty();

                    if inline_only {
                        if let Some(spans) = active_spans_mut(&mut frames) {
                            style.push_span(spans, &finished.alt, false);
                        }
                    } else {
                        match frames.last_mut() {
                            Some(ActiveFrame::Paragraph {
                                spans,
                                quote_depth: pqd,
                                start_line: p_start,
                            }) => {
                                if !spans.is_empty() {
                                    let kind = if *pqd > 0 {
                                        BlockKind::BlockQuote
                                    } else {
                                        BlockKind::Paragraph
                                    };
                                    blocks.push(DocBlock {
                                        kind,
                                        spans: mem::take(spans),
                                        quote_depth: *pqd,
                                        start_line: *p_start,
                                        end_line: finished.start_line.max(*p_start),
                                    });
                                }
                                *p_start = ev_end_line;
                                blocks.push(DocBlock {
                                    kind: BlockKind::Image {
                                        url: finished.url,
                                        alt: finished.alt,
                                    },
                                    spans: Vec::new(),
                                    quote_depth: 0,
                                    start_line: finished.start_line,
                                    end_line: ev_end_line.max(finished.start_line),
                                });
                            }
                            Some(ActiveFrame::ListItem(_)) => {
                                flush_pending_list_item(
                                    &mut frames,
                                    &mut blocks,
                                    finished.start_line,
                                );
                                blocks.push(DocBlock {
                                    kind: BlockKind::Image {
                                        url: finished.url,
                                        alt: finished.alt,
                                    },
                                    spans: Vec::new(),
                                    quote_depth: 0,
                                    start_line: finished.start_line,
                                    end_line: ev_end_line.max(finished.start_line),
                                });
                            }
                            _ => {
                                blocks.push(DocBlock {
                                    kind: BlockKind::Image {
                                        url: finished.url,
                                        alt: finished.alt,
                                    },
                                    spans: Vec::new(),
                                    quote_depth: 0,
                                    start_line: finished.start_line,
                                    end_line: ev_end_line.max(finished.start_line),
                                });
                            }
                        }
                    }
                    continue;
                }
                Event::Text(t) | Event::Code(t) => {
                    cap.alt.push_str(&t);
                    continue;
                }
                Event::SoftBreak => {
                    cap.alt.push(' ');
                    continue;
                }
                Event::HardBreak => {
                    cap.alt.push('\n');
                    continue;
                }
                _ => continue,
            }
        }

        // 2. 常规事件流分发
        match event {
            Event::Start(tag) => match tag {
                Tag::Heading { level, .. } => {
                    flush_pending_list_item(
                        &mut frames,
                        &mut blocks,
                        ev_start_line.saturating_sub(1),
                    );
                    frames.push(ActiveFrame::Heading {
                        level: heading_level_to_u8(level),
                        spans: Vec::new(),
                        start_line: ev_start_line,
                    });
                }
                Tag::Paragraph => {
                    if let Some(ActiveFrame::ListItem(item)) = frames.last_mut()
                        && !item.emitted
                        && !item.first_para_done
                    {
                        item.in_first_para = true;
                    } else if !matches!(frames.last(), Some(ActiveFrame::Table(_))) {
                        flush_pending_list_item(
                            &mut frames,
                            &mut blocks,
                            ev_start_line.saturating_sub(1),
                        );
                        frames.push(ActiveFrame::Paragraph {
                            spans: Vec::new(),
                            quote_depth: quote_depth.min(u16::MAX as usize) as u16,
                            start_line: ev_start_line,
                        });
                    }
                }
                Tag::BlockQuote(_) => {
                    flush_pending_list_item(
                        &mut frames,
                        &mut blocks,
                        ev_start_line.saturating_sub(1),
                    );
                    quote_depth += 1;
                }
                Tag::CodeBlock(kind) => {
                    flush_pending_list_item(
                        &mut frames,
                        &mut blocks,
                        ev_start_line.saturating_sub(1),
                    );
                    let language = match kind {
                        CodeBlockKind::Indented => None,
                        CodeBlockKind::Fenced(info) => info
                            .split(|c: char| c.is_whitespace() || c == ',')
                            .find(|s| !s.is_empty())
                            .map(str::to_string),
                    };
                    frames.push(ActiveFrame::CodeBlock {
                        language,
                        buf: String::new(),
                        start_line: ev_start_line,
                    });
                }
                Tag::HtmlBlock => {
                    flush_pending_list_item(
                        &mut frames,
                        &mut blocks,
                        ev_start_line.saturating_sub(1),
                    );
                    frames.push(ActiveFrame::HtmlBlock {
                        buf: String::new(),
                        start_line: ev_start_line,
                    });
                }
                Tag::List(start_opt) => {
                    flush_pending_list_item(
                        &mut frames,
                        &mut blocks,
                        ev_start_line.saturating_sub(1),
                    );
                    list_stack.push(ListState {
                        ordered: start_opt.is_some(),
                        next_index: start_opt.unwrap_or(0),
                    });
                }
                Tag::Item => {
                    let depth = list_stack
                        .len()
                        .saturating_sub(1)
                        .min(u16::MAX as usize) as u16;
                    let (ordered, index) = if let Some(state) = list_stack.last_mut() {
                        if state.ordered {
                            let current = state.next_index;
                            state.next_index = state.next_index.saturating_add(1);
                            (true, Some(current))
                        } else {
                            (false, None)
                        }
                    } else {
                        (false, None)
                    };
                    frames.push(ActiveFrame::ListItem(ListItemFrame {
                        ordered,
                        index,
                        depth,
                        task: None,
                        spans: Vec::new(),
                        emitted: false,
                        in_first_para: false,
                        first_para_done: false,
                        start_line: ev_start_line,
                        end_line: ev_end_line,
                    }));
                }
                Tag::Table(alignments) => {
                    flush_pending_list_item(
                        &mut frames,
                        &mut blocks,
                        ev_start_line.saturating_sub(1),
                    );
                    frames.push(ActiveFrame::Table(TableFrame {
                        alignments: alignments.into_iter().map(map_table_align).collect(),
                        rows: Vec::new(),
                        current_row: None,
                        current_cell: None,
                        start_line: ev_start_line,
                        end_line: ev_end_line,
                    }));
                }
                Tag::TableHead | Tag::TableRow => {
                    if let Some(ActiveFrame::Table(table)) = frames.last_mut() {
                        table.current_row = Some(Vec::new());
                    }
                }
                Tag::TableCell => {
                    if let Some(ActiveFrame::Table(table)) = frames.last_mut() {
                        table.current_cell = Some(Vec::new());
                    }
                }
                Tag::Emphasis => {
                    style.italic_depth += 1;
                }
                Tag::Strong => {
                    style.bold_depth += 1;
                }
                Tag::Strikethrough => {
                    style.strike_depth += 1;
                }
                Tag::Link { dest_url, .. } => {
                    style.link_stack.push(dest_url.into_string());
                }
                Tag::Image { dest_url, .. } => {
                    image_capture = Some(ImageCapture {
                        url: dest_url.into_string(),
                        alt: String::new(),
                        nested_depth: 1,
                        start_line: ev_start_line,
                    });
                }
                Tag::FootnoteDefinition(_)
                | Tag::MetadataBlock(_)
                | Tag::DefinitionList
                | Tag::DefinitionListTitle
                | Tag::DefinitionListDefinition
                | Tag::Superscript
                | Tag::Subscript => {}
            },

            Event::End(tag_end) => match tag_end {
                TagEnd::Heading(_) => {
                    if let Some(ActiveFrame::Heading {
                        level,
                        spans,
                        start_line,
                    }) = frames.pop()
                    {
                        blocks.push(DocBlock {
                            kind: BlockKind::Heading { level },
                            spans,
                            quote_depth: quote_depth.min(u16::MAX as usize) as u16,
                            start_line,
                            end_line: ev_end_line.max(start_line),
                        });
                    }
                }
                TagEnd::Paragraph => {
                    if let Some(ActiveFrame::ListItem(item)) = frames.last_mut() {
                        if item.in_first_para {
                            item.in_first_para = false;
                            item.first_para_done = true;
                            item.end_line = ev_end_line.max(item.start_line);
                            // 若首段中间曾因 BlockKind::Image 提前输出过该 ListItem，
                            // 则图片后方残留的同段文本仍须保留为独立段落，绝不静默丢失。
                            if item.emitted && !item.spans.is_empty() {
                                blocks.push(DocBlock {
                                    kind: if quote_depth > 0 {
                                        BlockKind::BlockQuote
                                    } else {
                                        BlockKind::Paragraph
                                    },
                                    spans: mem::take(&mut item.spans),
                                    quote_depth: quote_depth.min(u16::MAX as usize) as u16,
                                    start_line: item.start_line,
                                    end_line: ev_end_line.max(item.start_line),
                                });
                            }
                        }
                    } else if matches!(frames.last(), Some(ActiveFrame::Paragraph { .. }))
                        && let Some(ActiveFrame::Paragraph {
                            spans,
                            quote_depth: pqd,
                            start_line,
                        }) = frames.pop()
                        && !spans.is_empty()
                    {
                        blocks.push(DocBlock {
                            kind: if pqd > 0 {
                                BlockKind::BlockQuote
                            } else {
                                BlockKind::Paragraph
                            },
                            spans,
                            quote_depth: pqd,
                            start_line,
                            end_line: ev_end_line.max(start_line),
                        });
                    }
                }
                TagEnd::BlockQuote(_) => {
                    quote_depth = quote_depth.saturating_sub(1);
                }
                TagEnd::CodeBlock => {
                    if let Some(ActiveFrame::CodeBlock {
                        language,
                        buf,
                        start_line,
                    }) = frames.pop()
                    {
                        let clean_code = strip_trailing_single_newline(&buf);
                        let spans = highlight_code(language.as_deref(), clean_code);
                        blocks.push(DocBlock {
                            kind: BlockKind::CodeBlock { language },
                            spans,
                            quote_depth: quote_depth.min(u16::MAX as usize) as u16,
                            start_line,
                            end_line: ev_end_line.max(start_line),
                        });
                    }
                }
                TagEnd::HtmlBlock => {
                    if let Some(ActiveFrame::HtmlBlock { buf, start_line }) = frames.pop()
                        && let Some(text) = sanitize_html_block_to_plain_text(&buf)
                    {
                        blocks.push(DocBlock {
                            kind: BlockKind::Paragraph,
                            spans: vec![RichSpan {
                                text,
                                bold: false,
                                italic: false,
                                strike: false,
                                code: false,
                                link_url: None,
                                highlight: None,
                            }],
                            quote_depth: quote_depth.min(u16::MAX as usize) as u16,
                            start_line,
                            end_line: ev_end_line.max(start_line),
                        });
                    }
                }
                TagEnd::List(_) => {
                    list_stack.pop();
                }
                TagEnd::Item => {
                    if let Some(ActiveFrame::ListItem(item)) = frames.pop() {
                        if !item.emitted {
                            blocks.push(DocBlock {
                                kind: BlockKind::ListItem {
                                    ordered: item.ordered,
                                    index: item.index,
                                    depth: item.depth,
                                    task: item.task,
                                },
                                spans: item.spans,
                                quote_depth: quote_depth.min(u16::MAX as usize) as u16,
                                start_line: item.start_line,
                                end_line: item.end_line.max(item.start_line),
                            });
                        } else if !item.spans.is_empty() {
                            blocks.push(DocBlock {
                                kind: if quote_depth > 0 {
                                    BlockKind::BlockQuote
                                } else {
                                    BlockKind::Paragraph
                                },
                                spans: item.spans,
                                quote_depth: quote_depth.min(u16::MAX as usize) as u16,
                                start_line: item.start_line,
                                end_line: ev_end_line.max(item.start_line),
                            });
                        }
                    }
                }
                TagEnd::TableCell => {
                    if let Some(ActiveFrame::Table(table)) = frames.last_mut() {
                        let cell = table.current_cell.take().unwrap_or_default();
                        if let Some(row) = table.current_row.as_mut() {
                            row.push(cell);
                        }
                    }
                }
                TagEnd::TableHead | TagEnd::TableRow => {
                    if let Some(ActiveFrame::Table(table)) = frames.last_mut()
                        && let Some(row) = table.current_row.take()
                    {
                        table.rows.push(row);
                    }
                }
                TagEnd::Table => {
                    if let Some(ActiveFrame::Table(table)) = frames.pop() {
                        blocks.push(DocBlock {
                            kind: BlockKind::Table {
                                alignments: table.alignments,
                                rows: table.rows,
                            },
                            spans: Vec::new(),
                            quote_depth: quote_depth.min(u16::MAX as usize) as u16,
                            start_line: table.start_line,
                            end_line: ev_end_line.max(table.end_line).max(table.start_line),
                        });
                    }
                }
                TagEnd::Emphasis => {
                    style.italic_depth = style.italic_depth.saturating_sub(1);
                }
                TagEnd::Strong => {
                    style.bold_depth = style.bold_depth.saturating_sub(1);
                }
                TagEnd::Strikethrough => {
                    style.strike_depth = style.strike_depth.saturating_sub(1);
                }
                TagEnd::Link => {
                    style.link_stack.pop();
                }
                TagEnd::Image
                | TagEnd::FootnoteDefinition
                | TagEnd::MetadataBlock(_)
                | TagEnd::DefinitionList
                | TagEnd::DefinitionListTitle
                | TagEnd::DefinitionListDefinition
                | TagEnd::Superscript
                | TagEnd::Subscript => {}
            },

            Event::Text(text) => {
                if let Some(
                    ActiveFrame::CodeBlock { buf, .. } | ActiveFrame::HtmlBlock { buf, .. },
                ) = frames.last_mut()
                {
                    buf.push_str(&text);
                } else if let Some(spans) = active_spans_mut(&mut frames) {
                    style.push_span(spans, &text, false);
                }
            }

            Event::Code(code) => {
                if let Some(spans) = active_spans_mut(&mut frames) {
                    style.push_span(spans, &code, true);
                }
            }

            Event::Html(html) => {
                if let Some(ActiveFrame::HtmlBlock { buf, .. }) = frames.last_mut() {
                    buf.push_str(&html);
                } else if let Some(spans) = active_spans_mut(&mut frames)
                    && matches!(classify_inline_html(&html), InlineHtmlAction::PlainText)
                {
                    style.push_span(spans, &html, false);
                }
            }

            Event::InlineHtml(html) => {
                if let Some(spans) = active_spans_mut(&mut frames) {
                    match classify_inline_html(&html) {
                        InlineHtmlAction::Newline => style.push_span(spans, "\n", false),
                        InlineHtmlAction::Ignore => {}
                        InlineHtmlAction::PlainText => style.push_span(spans, &html, false),
                    }
                }
            }

            Event::SoftBreak => {
                if let Some(
                    ActiveFrame::CodeBlock { buf, .. } | ActiveFrame::HtmlBlock { buf, .. },
                ) = frames.last_mut()
                {
                    buf.push('\n');
                } else if let Some(spans) = active_spans_mut(&mut frames) {
                    style.push_span(spans, " ", false);
                }
            }

            Event::HardBreak => {
                if let Some(
                    ActiveFrame::CodeBlock { buf, .. } | ActiveFrame::HtmlBlock { buf, .. },
                ) = frames.last_mut()
                {
                    buf.push('\n');
                } else if let Some(spans) = active_spans_mut(&mut frames) {
                    style.push_span(spans, "\n", false);
                }
            }

            Event::Rule => {
                flush_pending_list_item(
                    &mut frames,
                    &mut blocks,
                    ev_start_line.saturating_sub(1),
                );
                blocks.push(DocBlock {
                    kind: BlockKind::ThematicBreak,
                    spans: Vec::new(),
                    quote_depth: 0,
                    start_line: ev_start_line,
                    end_line: ev_end_line.max(ev_start_line),
                });
            }

            Event::TaskListMarker(checked) => {
                if let Some(ActiveFrame::ListItem(item)) = frames.last_mut()
                    && item.task.is_none()
                {
                    item.task = Some(checked);
                }
            }

            Event::FootnoteReference(text)
            | Event::InlineMath(text)
            | Event::DisplayMath(text) => {
                if let Some(spans) = active_spans_mut(&mut frames) {
                    style.push_span(spans, &text, false);
                }
            }
        }
    }

    ParsedDocument {
        blocks,
        total_lines,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain_text_of(spans: &[RichSpan]) -> String {
        spans.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn test_empty_and_whitespace_inputs_produce_empty_document() {
        assert!(parse_markdown("").blocks.is_empty());
        assert!(parse_markdown("   \n\t\r\n  ").blocks.is_empty());
    }

    #[test]
    fn test_headings_h1_to_h6_with_inline_formatting() {
        let md = "# H1 **Bold** *Italic* `code` [link](https://example.com)\n\
                  ## H2\n### H3\n#### H4\n##### H5\n###### H6\n";
        let doc = parse_markdown(md);
        assert_eq!(doc.blocks.len(), 6);

        for (idx, level) in (1u8..=6u8).enumerate() {
            assert_eq!(doc.blocks[idx].kind, BlockKind::Heading { level });
        }

        let h1_spans = &doc.blocks[0].spans;
        assert!(h1_spans.iter().any(|s| s.bold && s.text == "Bold"));
        assert!(h1_spans.iter().any(|s| s.italic && s.text == "Italic"));
        assert!(h1_spans.iter().any(|s| s.code && s.text == "code"));
        assert!(
            h1_spans
                .iter()
                .any(|s| s.link_url.as_deref() == Some("https://example.com") && s.text == "link")
        );
    }

    #[test]
    fn test_paragraph_soft_hard_break_strike_and_span_merging() {
        let md = "第一行\n第二行  \n第三行~~删除~~";
        let doc = parse_markdown(md);
        assert_eq!(doc.blocks.len(), 1);
        assert_eq!(doc.blocks[0].kind, BlockKind::Paragraph);

        let spans = &doc.blocks[0].spans;
        assert_eq!(plain_text_of(spans), "第一行 第二行\n第三行删除");
        // 同样式文本（第一行 + " " + 第二行 + "\n" + 第三行）应自动合并为一个 span
        assert_eq!(spans.len(), 2);
        assert!(!spans[0].strike);
        assert!(spans[1].strike && spans[1].text == "删除");
    }

    #[test]
    fn test_nested_ordered_unordered_and_task_lists() {
        let md = "\
3. 第三项
   - 子项 A
     1. 孙项 1
4. 第四项

- [ ] 待办紧列表
- [x] 已完成紧列表

- [ ] 空任务或松散

  第二段文字

- [ ]
";
        let doc = parse_markdown(md);

        let list_items: Vec<_> = doc
            .blocks
            .iter()
            .filter_map(|b| match &b.kind {
                BlockKind::ListItem {
                    ordered,
                    index,
                    depth,
                    task,
                } => Some((*ordered, *index, *depth, *task, plain_text_of(&b.spans))),
                _ => None,
            })
            .collect();

        assert_eq!(
            list_items,
            vec![
                (true, Some(3), 0, None, "第三项".to_string()),
                (false, None, 1, None, "子项 A".to_string()),
                (true, Some(1), 2, None, "孙项 1".to_string()),
                (true, Some(4), 0, None, "第四项".to_string()),
                (false, None, 0, Some(false), "待办紧列表".to_string()),
                (false, None, 0, Some(true), "已完成紧列表".to_string()),
                (false, None, 0, Some(false), "空任务或松散".to_string()),
                (false, None, 0, Some(false), "".to_string()),
            ]
        );

        assert!(doc.blocks.iter().any(|b| {
            b.kind == BlockKind::Paragraph && plain_text_of(&b.spans) == "第二段文字"
        }));
    }

    #[test]
    fn test_blockquotes_nested_and_with_code() {
        let md = "\
> 第一层引用
>
> > 第二层嵌套引用
>
> ```rust
> let x = 1;
> ```
";
        let doc = parse_markdown(md);
        assert!(doc.blocks.iter().any(|b| {
            b.kind == BlockKind::BlockQuote && plain_text_of(&b.spans) == "第一层引用"
        }));
        assert!(doc.blocks.iter().any(|b| {
            b.kind == BlockKind::BlockQuote && plain_text_of(&b.spans) == "第二层嵌套引用"
        }));
        assert!(doc.blocks.iter().any(|b| {
            matches!(&b.kind, BlockKind::CodeBlock { language } if language.as_deref() == Some("rust"))
        }));
    }

    #[test]
    fn test_gfm_table_alignments_rich_cells_and_missing_cell_padding() {
        let md = "\
| 默认 | 左对齐 | 居中 | 右对齐 |
| --- | :--- | :---: | ---: |
| **粗体** | `code` | [链接](https://a.test) | ~~删~~ |
| 缺列 |
";
        let doc = parse_markdown(md);
        assert_eq!(doc.blocks.len(), 1);
        match &doc.blocks[0].kind {
            BlockKind::Table { alignments, rows } => {
                assert_eq!(
                    alignments,
                    &vec![
                        TableAlign::None,
                        TableAlign::Left,
                        TableAlign::Center,
                        TableAlign::Right,
                    ]
                );
                assert_eq!(rows.len(), 3);
                assert_eq!(plain_text_of(&rows[0][0]), "默认");
                assert_eq!(plain_text_of(&rows[0][3]), "右对齐");
                assert!(rows[1][0][0].bold);
                assert!(rows[1][1][0].code);
                assert_eq!(
                    rows[1][2][0].link_url.as_deref(),
                    Some("https://a.test")
                );
                assert!(rows[1][3][0].strike);
                // pulldown-cmark 自动补齐缺失单元格
                assert_eq!(rows[2].len(), 4);
                assert_eq!(plain_text_of(&rows[2][0]), "缺列");
            }
            other => panic!("期望 Table，实际为: {other:?}"),
        }
    }

    #[test]
    fn test_fenced_and_indented_code_blocks_and_thematic_break() {
        let md = "\
```rust,no_run
fn main() {
    let a = 1;
}
```

---

    indented_line();
";
        let doc = parse_markdown(md);
        assert_eq!(doc.blocks.len(), 3);

        match &doc.blocks[0].kind {
            BlockKind::CodeBlock { language } => assert_eq!(language.as_deref(), Some("rust")),
            other => panic!("期望围栏代码块，实际: {other:?}"),
        }
        assert!(!plain_text_of(&doc.blocks[0].spans).ends_with('\n'));
        assert!(
            doc.blocks[0]
                .spans
                .iter()
                .any(|s| s.highlight == Some(TokenKind::Keyword) && s.text == "fn")
        );

        assert_eq!(doc.blocks[1].kind, BlockKind::ThematicBreak);

        match &doc.blocks[2].kind {
            BlockKind::CodeBlock { language } => assert!(language.is_none()),
            other => panic!("期望缩进代码块，实际: {other:?}"),
        }
        assert_eq!(plain_text_of(&doc.blocks[2].spans), "indented_line();");
    }

    #[test]
    fn test_images_standalone_inline_split_list_and_badge_link() {
        let md = "\
![独立图片](a.png)

前文 ![内嵌图](b.png) 后文

[![徽章](badge.svg)](https://repo.example)

- 列表前 ![列表图](c.png) 列表后
";
        let doc = parse_markdown(md);

        assert!(doc.blocks.iter().any(|b| {
            b.kind == BlockKind::Image {
                url: "a.png".to_string(),
                alt: "独立图片".to_string(),
            }
        }));
        assert!(doc.blocks.iter().any(|b| {
            b.kind == BlockKind::Paragraph && plain_text_of(&b.spans) == "前文 "
        }));
        assert!(doc.blocks.iter().any(|b| {
            b.kind == BlockKind::Image {
                url: "b.png".to_string(),
                alt: "内嵌图".to_string(),
            }
        }));
        assert!(doc.blocks.iter().any(|b| {
            b.kind == BlockKind::Paragraph && plain_text_of(&b.spans) == " 后文"
        }));
        assert!(doc.blocks.iter().any(|b| {
            b.kind == BlockKind::Paragraph
                && b.spans.iter().any(|s| {
                    s.link_url.as_deref() == Some("https://repo.example") && s.text == "徽章"
                })
        }));
        assert!(doc.blocks.iter().any(|b| {
            matches!(b.kind, BlockKind::ListItem { .. }) && plain_text_of(&b.spans) == "列表前 "
        }));
        assert!(doc.blocks.iter().any(|b| {
            b.kind == BlockKind::Paragraph && plain_text_of(&b.spans) == " 列表后"
        }));
    }

    #[test]
    fn test_raw_html_safely_degraded_without_script_execution_or_text_loss() {
        let md = "\
<script>alert(1)</script>

段落内<kbd>Ctrl+C</kbd>与a<br>b<!-- 隐藏注释 -->

<div>安全容器文字</div>
";
        let doc = parse_markdown(md);

        assert!(doc.blocks.iter().any(|b| {
            b.kind == BlockKind::Paragraph && plain_text_of(&b.spans).contains("alert(1)")
        }));
        assert!(doc.blocks.iter().any(|b| {
            b.kind == BlockKind::Paragraph && plain_text_of(&b.spans) == "段落内Ctrl+C与a\nb"
        }));
        assert!(doc.blocks.iter().any(|b| {
            b.kind == BlockKind::Paragraph && plain_text_of(&b.spans) == "安全容器文字"
        }));
        assert!(!doc.blocks.iter().any(|b| plain_text_of(&b.spans).contains("隐藏注释")));
    }

    #[test]
    fn test_deep_nesting_500_levels_does_not_overflow_stack() {
        let deep_quotes = format!("{}深层引用文本", "> ".repeat(500));
        let doc_q = parse_markdown(&deep_quotes);
        assert_eq!(doc_q.blocks.len(), 1);
        assert_eq!(doc_q.blocks[0].kind, BlockKind::BlockQuote);
        assert_eq!(plain_text_of(&doc_q.blocks[0].spans), "深层引用文本");

        let mut deep_lists = String::new();
        for depth in 0..500usize {
            for _ in 0..depth {
                deep_lists.push_str("  ");
            }
            deep_lists.push_str("- item\n");
        }
        let doc_l = parse_markdown(&deep_lists);
        assert!(!doc_l.blocks.is_empty());
    }

    #[test]
    fn test_source_line_mapping_and_outline_extraction() {
        let md = "# 一级标题\n\n第一段文字\n第二行\n\n## 二级 `代码` 标题\n\n```rust\nlet x = 1;\nlet y = 2;\n```\n\n### 三级标题\n";
        let doc = parse_markdown(md);

        assert_eq!(doc.blocks[0].start_line, 0);
        assert_eq!(doc.blocks[1].start_line, 2);
        assert_eq!(doc.blocks[1].end_line, 3);
        assert_eq!(doc.blocks[2].start_line, 5);
        assert_eq!(doc.blocks[3].start_line, 7);
        assert_eq!(doc.blocks[3].end_line, 10);

        let outline = doc.outline();
        assert_eq!(outline.len(), 3);
        assert_eq!(outline[0].level, 1);
        assert_eq!(outline[0].title, "一级标题");
        assert_eq!(outline[0].start_line, 0);
        assert_eq!(outline[1].level, 2);
        assert_eq!(outline[1].title, "二级 代码 标题");
        assert_eq!(outline[1].start_line, 5);
        assert_eq!(outline[2].level, 3);
        assert_eq!(outline[2].title, "三级标题");
    }
}
