//! Simple markdown renderer for terminal output

use once_cell::sync::Lazy;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use syntect::{
    easy::HighlightLines,
    highlighting::{Style as SyntectStyle, ThemeSet},
    parsing::SyntaxSet,
};

// Lazy-loaded syntax highlighting resources
static SYNTAX_SET: Lazy<SyntaxSet> = Lazy::new(SyntaxSet::load_defaults_newlines);
static THEME_SET: Lazy<ThemeSet> = Lazy::new(ThemeSet::load_defaults);

// Color scheme for markdown elements
mod colors {
    use super::Color;
    pub const BOLD: Color = Color::White;
    pub const ITALIC: Color = Color::Rgb(180, 180, 180);
    pub const CODE: Color = Color::Rgb(230, 200, 150);
    pub const CODE_BG: Color = Color::Rgb(40, 44, 52);
    pub const LINK: Color = Color::Rgb(88, 166, 255);
    pub const HEADER: Color = Color::Rgb(88, 166, 255);
    pub const BLOCKQUOTE: Color = Color::Rgb(150, 150, 170); // Slightly muted for blockquotes
    pub const MUTED: Color = Color::Rgb(125, 133, 144);
    pub const LINE_NUMBER: Color = Color::Rgb(80, 90, 100); // Subtle line numbers
}

/// Render markdown text into styled terminal lines
pub fn render_markdown(text: &str, base_style: Style) -> Vec<Line<'static>> {
    let renderer = MarkdownRenderer::new(base_style);
    renderer.render(text)
}

/// Highlight JSON text with syntax coloring
pub fn highlight_json(text: &str) -> Vec<Span<'static>> {
    let syntax = SYNTAX_SET
        .find_syntax_by_token("json")
        .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text());
    let theme = &THEME_SET.themes["base16-ocean.dark"];
    let mut highlighter = HighlightLines::new(syntax, theme);
    
    // Highlight the text as a single line (for inline display)
    match highlighter.highlight_line(text, &SYNTAX_SET) {
        Ok(ranges) => ranges
            .into_iter()
            .map(|(style, text)| Span::styled(text.to_string(), syntect_to_ratatui_style(style)))
            .collect(),
        Err(_) => vec![Span::styled(text.to_string(), Style::default().fg(colors::CODE))],
    }
}

struct MarkdownRenderer {
    base_style: Style,
    lines: Vec<Line<'static>>,
    in_code_block: bool,
    code_block_lang: Option<String>,
    code_block_lines: Vec<String>,
}

impl MarkdownRenderer {
    fn new(base_style: Style) -> Self {
        Self {
            base_style,
            lines: Vec::new(),
            in_code_block: false,
            code_block_lang: None,
            code_block_lines: Vec::new(),
        }
    }

    fn render(mut self, text: &str) -> Vec<Line<'static>> {
        for line in text.lines() {
            self.process_line(line);
        }
        // Handle unclosed code block
        if self.in_code_block {
            self.flush_code_block();
        }
        self.lines
    }

    fn process_line(&mut self, line: &str) {
        // Code block handling
        if line.starts_with("```") {
            if self.in_code_block {
                self.flush_code_block();
            } else {
                self.in_code_block = true;
                // Extract language hint (e.g., ```rust, ```python)
                let lang = line.trim_start_matches('`').trim();
                self.code_block_lang = if lang.is_empty() {
                    None
                } else {
                    Some(lang.to_lowercase())
                };
            }
            return;
        }

        if self.in_code_block {
            self.code_block_lines.push(line.to_string());
            return;
        }

        // Try each block-level handler in order
        if let Some(rendered) = self
            .try_header(line)
            .or_else(|| self.try_horizontal_rule(line))
            .or_else(|| self.try_list_item(line))
            .or_else(|| self.try_blockquote(line))
        {
            self.lines.push(rendered);
        } else {
            // Regular paragraph
            let spans = render_inline(line, self.base_style);
            self.lines.push(if spans.is_empty() {
                Line::from("")
            } else {
                Line::from(spans)
            });
        }
    }

    fn flush_code_block(&mut self) {
        let code_lines = std::mem::take(&mut self.code_block_lines);
        let lang = self.code_block_lang.take();

        // Try to get syntax highlighting
        let highlighted = self.highlight_code(&code_lines, lang.as_deref());

        // Add a subtle top border
        let border_style = Style::default().fg(colors::MUTED);
        let lang_label = lang.as_deref().unwrap_or("code");
        self.lines.push(Line::from(vec![
            Span::styled("╭─ ", border_style),
            Span::styled(
                lang_label.to_string(),
                Style::default()
                    .fg(colors::CODE)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {}", "─".repeat(50)), border_style),
        ]));

        // Add code lines with line numbers
        let num_lines = highlighted.len();
        let line_num_width = if num_lines >= 100 { 3 } else if num_lines >= 10 { 2 } else { 1 };
        
        for (i, line_spans) in highlighted.into_iter().enumerate() {
            let line_num = format!("{:>width$}│ ", i + 1, width = line_num_width);
            let mut spans = vec![
                Span::styled("│ ", border_style),
                Span::styled(line_num, Style::default().fg(colors::LINE_NUMBER)),
            ];
            spans.extend(line_spans);
            self.lines.push(Line::from(spans));
        }

        // Add bottom border
        self.lines.push(Line::from(Span::styled(
            format!("╰{}─", "─".repeat(55)),
            border_style,
        )));

        self.in_code_block = false;
    }

    fn highlight_code(&self, lines: &[String], lang: Option<&str>) -> Vec<Vec<Span<'static>>> {
        // Try to find syntax for the language
        let syntax = lang
            .and_then(|l| SYNTAX_SET.find_syntax_by_token(l))
            .or_else(|| lang.and_then(|l| SYNTAX_SET.find_syntax_by_extension(l)))
            .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text());

        // Get a dark theme for highlighting
        let theme = &THEME_SET.themes["base16-ocean.dark"];
        let mut highlighter = HighlightLines::new(syntax, theme);

        let mut result = Vec::new();

        for line in lines {
            let highlighted = highlighter.highlight_line(line, &SYNTAX_SET);

            match highlighted {
                Ok(ranges) => {
                    let spans: Vec<Span> = ranges
                        .into_iter()
                        .map(|(style, text)| {
                            Span::styled(text.to_string(), syntect_to_ratatui_style(style))
                        })
                        .collect();
                    result.push(spans);
                }
                Err(_) => {
                    // Fallback to plain text
                    result.push(vec![Span::styled(
                        line.clone(),
                        Style::default().fg(colors::CODE),
                    )]);
                }
            }
        }

        result
    }

    fn try_header(&self, line: &str) -> Option<Line<'static>> {
        if !line.starts_with('#') {
            return None;
        }

        let level = line.chars().take_while(|&c| c == '#').count();
        let content = line.trim_start_matches('#').trim();
        let prefix = match level {
            1 => "█ ",
            2 => "▌ ",
            3 => "▎ ",
            _ => "  ",
        };
        let style = Style::default()
            .fg(colors::HEADER)
            .add_modifier(Modifier::BOLD);

        Some(Line::from(vec![
            Span::styled(prefix, style),
            Span::styled(content.to_string(), style),
        ]))
    }

    fn try_horizontal_rule(&self, line: &str) -> Option<Line<'static>> {
        let trimmed = line.trim();
        if trimmed.len() < 3 {
            return None;
        }

        let first = trimmed.chars().next()?;
        if !matches!(first, '-' | '*' | '_') {
            return None;
        }

        let is_rule = trimmed.chars().all(|c| c == first || c.is_whitespace())
            && trimmed.chars().filter(|&c| c == first).count() >= 3;

        is_rule.then(|| {
            Line::from(Span::styled(
                "────────────────────────────────────────",
                Style::default().fg(colors::MUTED),
            ))
        })
    }

    fn try_list_item(&self, line: &str) -> Option<Line<'static>> {
        let trimmed = line.trim_start();
        let indent = " ".repeat(line.len() - trimmed.len());

        // Unordered list
        if let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("+ "))
        {
            let (marker, content) = parse_task_item(rest);
            return Some(self.make_list_line(&indent, marker, content));
        }

        // Ordered list (e.g., "1. ")
        if let Some(dot_pos) = trimmed.find(". ") {
            let number = &trimmed[..dot_pos];
            if number.chars().all(|c| c.is_ascii_digit()) {
                let content = &trimmed[dot_pos + 2..];
                let marker = format!("{}. ", number);
                return Some(self.make_list_line(&indent, &marker, content));
            }
        }

        None
    }

    fn make_list_line(&self, indent: &str, marker: &str, content: &str) -> Line<'static> {
        let mut spans = vec![
            Span::raw(indent.to_string()),
            Span::styled(marker.to_string(), Style::default().fg(colors::MUTED)),
        ];
        spans.extend(render_inline(content, self.base_style));
        Line::from(spans)
    }

    fn try_blockquote(&self, line: &str) -> Option<Line<'static>> {
        if !line.starts_with('>') {
            return None;
        }

        let content = line.trim_start_matches('>').trim_start();
        let mut spans = vec![Span::styled("│ ", Style::default().fg(colors::BLOCKQUOTE))];
        spans.extend(render_inline(content, self.base_style.fg(colors::BLOCKQUOTE)));
        Some(Line::from(spans))
    }
}

fn parse_task_item(text: &str) -> (&str, &str) {
    if let Some(rest) = text.strip_prefix("[ ] ") {
        ("☐ ", rest)
    } else if let Some(rest) = text
        .strip_prefix("[x] ")
        .or_else(|| text.strip_prefix("[X] "))
    {
        ("☑ ", rest)
    } else {
        ("• ", text)
    }
}

/// Render inline markdown (bold, italic, code, links)
fn render_inline(text: &str, base_style: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek().is_some() => {
                current.push(chars.next().unwrap());
            }
            '`' => {
                flush_current(&mut spans, &mut current, base_style);
                let code = collect_until_char(&mut chars, '`');
                spans.push(Span::styled(
                    code,
                    Style::default().fg(colors::CODE).bg(colors::CODE_BG),
                ));
            }
            '*' | '_' => {
                flush_current(&mut spans, &mut current, base_style);
                let is_bold = chars.peek() == Some(&c);
                if is_bold {
                    chars.next();
                    let delim = format!("{}{}", c, c);
                    let content = collect_until_str(&mut chars, &delim);
                    spans.push(Span::styled(
                        content,
                        base_style.fg(colors::BOLD).add_modifier(Modifier::BOLD),
                    ));
                } else {
                    let content = collect_until_char(&mut chars, c);
                    spans.push(Span::styled(
                        content,
                        base_style.fg(colors::ITALIC).add_modifier(Modifier::ITALIC),
                    ));
                }
            }
            '~' if chars.peek() == Some(&'~') => {
                chars.next();
                flush_current(&mut spans, &mut current, base_style);
                let content = collect_until_str(&mut chars, "~~");
                spans.push(Span::styled(
                    content,
                    base_style.add_modifier(Modifier::CROSSED_OUT),
                ));
            }
            '[' => {
                flush_current(&mut spans, &mut current, base_style);
                let link_text = collect_until_char(&mut chars, ']');
                if chars.peek() == Some(&'(') {
                    chars.next();
                    let url = collect_until_char(&mut chars, ')');
                    spans.push(Span::styled(
                        link_text.clone(),
                        Style::default()
                            .fg(colors::LINK)
                            .add_modifier(Modifier::UNDERLINED),
                    ));
                    if !url.is_empty() && url != link_text {
                        spans.push(Span::styled(
                            format!(" ({})", url),
                            Style::default().fg(colors::MUTED),
                        ));
                    }
                } else {
                    spans.push(Span::styled(format!("[{}]", link_text), base_style));
                }
            }
            'h' if current.is_empty() || current.ends_with(char::is_whitespace) => {
                // Potential URL start
                let mut url_candidate = String::from(c);
                while let Some(&next) = chars.peek() {
                    if next.is_whitespace()
                        || next == ')'
                        || next == ']'
                        || next == '>'
                        || next == '"'
                        || next == '\''
                    {
                        break;
                    }
                    url_candidate.push(chars.next().unwrap());
                }

                if is_url(&url_candidate) {
                    flush_current(&mut spans, &mut current, base_style);
                    spans.push(Span::styled(
                        url_candidate,
                        Style::default()
                            .fg(colors::LINK)
                            .add_modifier(Modifier::UNDERLINED),
                    ));
                } else {
                    current.push_str(&url_candidate);
                }
            }
            _ => current.push(c),
        }
    }

    flush_current(&mut spans, &mut current, base_style);
    spans
}

/// Check if a string looks like a URL
fn is_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://") || s.starts_with("ftp://")
}

fn flush_current(spans: &mut Vec<Span<'static>>, current: &mut String, style: Style) {
    if !current.is_empty() {
        spans.push(Span::styled(std::mem::take(current), style));
    }
}

fn collect_until_char(chars: &mut std::iter::Peekable<std::str::Chars>, delim: char) -> String {
    let mut result = String::new();
    for c in chars.by_ref() {
        if c == delim {
            break;
        }
        result.push(c);
    }
    result
}

fn collect_until_str(chars: &mut std::iter::Peekable<std::str::Chars>, delim: &str) -> String {
    let mut result = String::new();
    while chars.peek().is_some() {
        result.push(chars.next().unwrap());
        if result.ends_with(delim) {
            result.truncate(result.len() - delim.len());
            break;
        }
    }
    result
}

/// Convert syntect style to ratatui style
fn syntect_to_ratatui_style(style: SyntectStyle) -> Style {
    let fg = Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b);

    let mut ratatui_style = Style::default().fg(fg);

    // Apply font style modifiers
    if style
        .font_style
        .contains(syntect::highlighting::FontStyle::BOLD)
    {
        ratatui_style = ratatui_style.add_modifier(Modifier::BOLD);
    }
    if style
        .font_style
        .contains(syntect::highlighting::FontStyle::ITALIC)
    {
        ratatui_style = ratatui_style.add_modifier(Modifier::ITALIC);
    }
    if style
        .font_style
        .contains(syntect::highlighting::FontStyle::UNDERLINE)
    {
        ratatui_style = ratatui_style.add_modifier(Modifier::UNDERLINED);
    }

    ratatui_style
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_header_levels() {
        let lines = render_markdown("# H1\n## H2\n### H3", Style::default());
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn test_inline_formatting() {
        let spans = render_inline("Hello **bold** and *italic*", Style::default());
        assert!(spans.len() >= 4);
    }

    #[test]
    fn test_code_inline() {
        let spans = render_inline("Use `code` here", Style::default());
        assert_eq!(spans.len(), 3);
    }

    #[test]
    fn test_horizontal_rule() {
        let lines = render_markdown("---", Style::default());
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn test_code_block_with_syntax_highlighting() {
        let md = r#"```rust
fn main() {
    println!("Hello");
}
```"#;
        let lines = render_markdown(md, Style::default());
        // Should have: top border, 3 code lines, bottom border = 5 lines
        assert!(
            lines.len() >= 5,
            "Expected at least 5 lines, got {}",
            lines.len()
        );
    }

    #[test]
    fn test_code_block_without_language() {
        let md = "```\nsome code\n```";
        let lines = render_markdown(md, Style::default());
        assert!(lines.len() >= 3);
    }

    #[test]
    fn test_url_detection() {
        let spans = render_inline("Check https://example.com for more", Style::default());
        // Should have: "Check ", URL, " for more"
        assert!(
            spans.len() >= 3,
            "Expected at least 3 spans, got {}",
            spans.len()
        );
        // The URL should be styled as a link
        let url_span = spans.iter().find(|s| s.content.contains("https://"));
        assert!(url_span.is_some(), "URL should be detected");
    }

    #[test]
    fn test_url_at_end() {
        let spans = render_inline("Visit https://rust-lang.org", Style::default());
        assert!(spans.len() >= 2);
    }
}
