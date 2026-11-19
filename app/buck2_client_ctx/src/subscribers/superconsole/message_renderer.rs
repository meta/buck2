/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use crossterm::style::Color;
use crossterm::style::Stylize;
use superconsole::Line;
use superconsole::Span;

pub fn warning_styled(text: &str) -> buck2_error::Result<Line> {
    // cross term doesn't directly define orange as a color
    let orange = Color::Rgb {
        r: (244),
        g: (140),
        b: (40),
    };
    Ok(Line::from_iter([Span::new_styled(
        text.to_owned().with(orange),
    )?]))
}

pub fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    if text.len() <= max_width {
        return vec![text.to_owned()];
    }

    let mut lines = Vec::new();
    let mut current_line = String::new();

    for word in text.split_whitespace() {
        // If adding this word would exceed the width
        if !current_line.is_empty() && current_line.len() + 1 + word.len() > max_width {
            lines.push(current_line.clone());
            current_line.clear();
        }

        // TODO(junliqin): Handle single words longer than max_width

        // Add the word to current line
        if !current_line.is_empty() {
            current_line.push(' ');
        }
        current_line.push_str(word);
    }

    if !current_line.is_empty() {
        lines.push(current_line);
    }

    // Ensure we have at least one line
    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

pub fn render_rich_message_lines(
    header: &str,
    body: &str,
    footer: Option<&str>,
) -> buck2_error::Result<Vec<Line>> {
    render_rich_message_lines_impl(header, body, footer, true)
}

pub fn render_rich_message_lines_compact(
    header: &str,
    body: &str,
    footer: Option<&str>,
) -> buck2_error::Result<Vec<Line>> {
    render_rich_message_lines_impl(header, body, footer, false)
}

fn render_rich_message_lines_impl(
    header: &str,
    body: &str,
    footer: Option<&str>,
    section_padding: bool,
) -> buck2_error::Result<Vec<Line>> {
    const MIN_BOX_WIDTH: usize = 50;
    const MAX_BOX_WIDTH: usize = 120; // Maximum width to prevent overly wide boxes
    const SIDE_PADDING: usize = 2; // Space for single space padding on each side (║ ... ║)

    let mut lines = Vec::new();

    // Calculate optimal box width
    let content_width = [header.len(), body.len(), footer.map_or(0, |f| f.len())]
        .into_iter()
        .max()
        .unwrap_or(MIN_BOX_WIDTH)
        .clamp(MIN_BOX_WIDTH, MAX_BOX_WIDTH);

    // Wrap text for each section
    let header_lines = wrap_text(header, content_width);
    let body_lines = wrap_text(body, content_width);
    let footer_lines = footer.map(|f| wrap_text(f, content_width));

    // Top border with corners - border width = content_width + 2 (for spaces) + 2 (for ║ chars)
    let border_width = content_width + SIDE_PADDING;
    let top_border = format!("╔{}╗", "═".repeat(border_width));
    lines.push(warning_styled(&top_border)?);

    // Header with side borders (multiple lines if needed)
    for header_line in &header_lines {
        let header_padded = format!("║ {:<width$} ║", header_line, width = content_width);
        lines.push(warning_styled(&header_padded)?);
    }

    if section_padding {
        let empty_line = format!("║ {} ║", " ".repeat(content_width));
        lines.push(warning_styled(&empty_line)?);
    }

    // Body with side borders (multiple lines if needed)
    for body_line in &body_lines {
        let body_padded = format!("║ {:<width$} ║", body_line, width = content_width);
        lines.push(warning_styled(&body_padded)?);
    }

    if section_padding {
        let empty_line = format!("║ {} ║", " ".repeat(content_width));
        lines.push(warning_styled(&empty_line)?);
    }

    // Footer if present (multiple lines if needed)
    if let Some(footer_lines) = footer_lines {
        for footer_line in &footer_lines {
            let footer_padded = format!("║ {:<width$} ║", footer_line, width = content_width);
            lines.push(warning_styled(&footer_padded)?);
        }
    }

    // Bottom border with corners
    let bottom_border = format!("╚{}╝", "═".repeat(border_width));
    lines.push(warning_styled(&bottom_border)?);

    Ok(lines)
}
