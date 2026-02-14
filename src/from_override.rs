use email::account::config::AccountConfig;

const FROM_EMAIL: Option<&str> = option_env!("HIMALAYA_FROM_EMAIL");
const FROM_NAME: Option<&str> = option_env!("HIMALAYA_FROM_NAME");
const DOMAIN: Option<&str> = option_env!("HIMALAYA_DOMAIN");

const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(BASE64_ALPHABET[((triple >> 18) & 0x3F) as usize] as char);
        out.push(BASE64_ALPHABET[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(BASE64_ALPHABET[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(BASE64_ALPHABET[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// RFC 2047 Base64-encode a display name if it contains non-ASCII characters.
/// Pure ASCII names are returned as-is (quoted if they contain special chars).
fn rfc2047_encode_display_name(name: &str) -> String {
    if name.is_ascii() {
        // Quote if it contains RFC 5322 specials
        if name
            .bytes()
            .any(|b| b"\"(),.:;<>@[\\]".contains(&b) || b == b' ')
        {
            return format!("\"{}\"", name.replace('\\', "\\\\").replace('"', "\\\""));
        }
        return name.to_string();
    }

    // RFC 2047 encoded-word: =?charset?encoding?encoded-text?=
    // Max 75 chars per encoded-word. Prefix "=?UTF-8?B?" (10) + suffix "?=" (2) = 12 overhead.
    // So max base64 payload per word = 75 - 12 = 63 chars = 63 base64 chars.
    // 63 base64 chars encode floor(63/4)*3 = 45 bytes of input.
    const MAX_INPUT_BYTES: usize = 45;

    let bytes = name.as_bytes();
    let mut words: Vec<String> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        // Find a chunk boundary that doesn't split a UTF-8 character
        let mut end = (i + MAX_INPUT_BYTES).min(bytes.len());
        // Walk back to a UTF-8 character boundary
        while end > i && end < bytes.len() && (bytes[end] & 0xC0) == 0x80 {
            end -= 1;
        }
        let encoded = base64_encode(&bytes[i..end]);
        words.push(format!("=?UTF-8?B?{}?=", encoded));
        i = end;
    }

    words.join("\r\n ")
}

/// Scan address headers (From, To, Cc, Bcc, Reply-To) in a raw RFC 5322
/// message and RFC 2047-encode any display names that contain non-ASCII.
pub fn encode_address_headers(msg: &[u8]) -> Vec<u8> {
    const ADDR_HEADERS: &[&str] = &["from:", "to:", "cc:", "bcc:", "reply-to:"];

    let src = String::from_utf8_lossy(msg);
    let mut result = String::with_capacity(src.len() + 128);
    let mut lines = src.split_inclusive('\n').peekable();

    while let Some(line) = lines.next() {
        let lower = line.to_ascii_lowercase();
        let trimmed = line.trim_end_matches(|c| c == '\r' || c == '\n');

        // Check for header/body separator
        if trimmed.is_empty() {
            // Append blank line and all remaining body
            result.push_str(line);
            for rest in lines.by_ref() {
                result.push_str(rest);
            }
            break;
        }

        let is_addr_header = ADDR_HEADERS.iter().any(|h| lower.starts_with(h));
        if !is_addr_header {
            result.push_str(line);
            continue;
        }

        // Collect the full header value (including folded continuation lines)
        let colon_pos = line.find(':').unwrap();
        let header_name = &line[..=colon_pos]; // e.g. "From:" or "To:"
        let mut value = line[colon_pos + 1..].to_string();
        // Gather folded lines
        while let Some(next) = lines.peek() {
            if next.starts_with(' ') || next.starts_with('\t') {
                value.push_str(lines.next().unwrap());
            } else {
                break;
            }
        }

        // Strip trailing CRLF/LF from collected value
        let value_trimmed = value.trim_end_matches(|c| c == '\r' || c == '\n');

        let encoded_value = encode_address_list(value_trimmed);
        result.push_str(header_name);
        result.push_str(&encoded_value);
        result.push_str("\r\n");
    }

    result.into_bytes()
}

/// Encode display names in a comma-separated address list.
fn encode_address_list(value: &str) -> String {
    let mut out = String::new();
    let mut rest = value;

    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }

        if !out.is_empty() {
            out.push_str(", ");
        }

        // Already-encoded display name: =?...?= <addr>
        if rest.starts_with("=?") {
            // Find the closing ?= then look for <addr>
            if let Some(end) = find_encoded_word_end(rest) {
                let mut pos = end;
                // Skip whitespace between encoded-word and angle-addr
                while pos < rest.len() && rest.as_bytes()[pos] == b' ' {
                    pos += 1;
                }
                if pos < rest.len() && rest.as_bytes()[pos] == b'<' {
                    if let Some(gt) = rest[pos..].find('>') {
                        let token_end = pos + gt + 1;
                        out.push_str(rest[..token_end].trim());
                        rest = skip_comma(&rest[token_end..]);
                        continue;
                    }
                }
                // Encoded word without angle-addr — pass through to next comma
                let (token, remainder) = split_at_comma(rest);
                out.push_str(token.trim());
                rest = remainder;
                continue;
            }
        }

        // Quoted display name: "Name" <addr>
        if rest.starts_with('"') {
            if let Some(close_quote) = rest[1..].find('"') {
                let display = &rest[1..1 + close_quote];
                let after_quote = &rest[2 + close_quote..];
                let after_trimmed = after_quote.trim_start();
                if after_trimmed.starts_with('<') {
                    if let Some(gt) = after_trimmed.find('>') {
                        let addr = &after_trimmed[..=gt];
                        let encoded = rfc2047_encode_display_name(display);
                        out.push_str(&encoded);
                        out.push(' ');
                        out.push_str(addr);
                        let consumed = after_trimmed[gt + 1..].as_ptr() as usize
                            - rest.as_ptr() as usize;
                        rest = skip_comma(&rest[consumed..]);
                        continue;
                    }
                }
            }
            // Couldn't parse — pass through
            let (token, remainder) = split_at_comma(rest);
            out.push_str(token.trim());
            rest = remainder;
            continue;
        }

        // Bare angle-addr: <addr> or addr with no display name
        if rest.starts_with('<') {
            if let Some(gt) = rest.find('>') {
                out.push_str(rest[..=gt].trim());
                rest = skip_comma(&rest[gt + 1..]);
                continue;
            }
        }

        // "Display Name <addr>" pattern
        if let Some(lt) = rest.find('<') {
            if let Some(gt) = rest[lt..].find('>') {
                let display = rest[..lt].trim();
                let addr = &rest[lt..lt + gt + 1];
                if display.is_empty() {
                    out.push_str(addr);
                } else {
                    let encoded = rfc2047_encode_display_name(display);
                    out.push_str(&encoded);
                    out.push(' ');
                    out.push_str(addr);
                }
                rest = skip_comma(&rest[lt + gt + 1..]);
                continue;
            }
        }

        // Bare email address (no angle brackets)
        let (token, remainder) = split_at_comma(rest);
        out.push_str(token.trim());
        rest = remainder;
    }

    if value.starts_with(' ') {
        format!(" {}", out)
    } else {
        out
    }
}

fn find_encoded_word_end(s: &str) -> Option<usize> {
    // =?charset?encoding?text?=
    // Find "?=" after the opening "=?"
    let inner = &s[2..];
    // Need at least: charset ? encoding ? text ?=
    let mut q_count = 0;
    for (i, b) in inner.bytes().enumerate() {
        if b == b'?' {
            q_count += 1;
            if q_count >= 3 && i + 1 < inner.len() && inner.as_bytes()[i + 1] == b'=' {
                return Some(2 + i + 2); // past the "?="
            }
        }
    }
    None
}

fn split_at_comma(s: &str) -> (&str, &str) {
    match s.find(',') {
        Some(pos) => (&s[..pos], &s[pos + 1..]),
        None => (s, ""),
    }
}

fn skip_comma(s: &str) -> &str {
    let s = s.trim_start();
    s.strip_prefix(',').unwrap_or(s)
}

/// Override `AccountConfig` email/display_name with compile-time values.
pub fn apply_from_override(config: &mut AccountConfig) {
    if let Some(email) = FROM_EMAIL {
        config.email = email.to_owned();
    }
    if let Some(name) = FROM_NAME {
        config.display_name = Some(name.to_owned());
    }
}

/// Replace the `From:` header in a raw RFC 5322 message with the
/// compile-time override value. Returns the original bytes unchanged
/// when no override is compiled in.
pub fn override_from_in_raw_message(msg: &[u8]) -> Vec<u8> {
    let email = match FROM_EMAIL {
        Some(e) => e,
        None => return msg.to_vec(),
    };

    let replacement = match FROM_NAME {
        Some(name) => format!("From: {} <{}>\r\n", rfc2047_encode_display_name(name), email),
        None => format!("From: {}\r\n", email),
    };

    let src = String::from_utf8_lossy(msg);

    // Find the From: header (case-insensitive).
    // It may span multiple lines (folded headers: continuation lines
    // start with whitespace).
    let mut result = String::with_capacity(src.len());
    let mut lines = src.split_inclusive('\n');
    let mut in_from_header = false;
    let mut from_replaced = false;

    while let Some(line) = lines.next() {
        if in_from_header {
            // Continuation line starts with space or tab
            if line.starts_with(' ') || line.starts_with('\t') {
                // Skip folded continuation of From header
                continue;
            }
            // No longer in the From header
            in_from_header = false;
            result.push_str(line);
        } else if !from_replaced
            && (line.starts_with("From:") || line.starts_with("from:") || {
                let lower = line.to_ascii_lowercase();
                lower.starts_with("from:")
            })
        {
            in_from_header = true;
            from_replaced = true;
            result.push_str(&replacement);
        } else if line.is_empty() || line == "\n" || line == "\r\n" {
            // We've reached the header/body separator without finding a
            // From header — insert one before the blank line.
            if !from_replaced {
                from_replaced = true;
                result.push_str(&replacement);
            }
            result.push_str(line);
            // Append the rest of the message as-is (body).
            for rest in lines.by_ref() {
                result.push_str(rest);
            }
            break;
        } else {
            result.push_str(line);
        }
    }

    // If we never hit a blank separator (unusual), append replacement
    if !from_replaced {
        result.push_str(&replacement);
    }

    result.into_bytes()
}

const EXCLUDED_RECIPIENTS: &[&str] = &[
    "ludo@viteunetable.com",
];

const CC_EMAIL: &str = "ludo@hey.com";

/// Extract the email address portion from an address token like
/// `Display Name <addr@example.com>` or bare `addr@example.com`.
fn extract_email(token: &str) -> &str {
    let t = token.trim();
    if let Some(lt) = t.find('<') {
        if let Some(gt) = t[lt..].find('>') {
            return &t[lt + 1..lt + gt];
        }
    }
    t
}

/// Return true if `token` contains any of the excluded email addresses.
fn is_excluded(token: &str) -> bool {
    let email = extract_email(token).trim();
    EXCLUDED_RECIPIENTS
        .iter()
        .any(|excl| email.eq_ignore_ascii_case(excl))
}

/// Strip excluded recipients from To, Cc, and Bcc headers in a raw
/// RFC 5322 message (`\r\n` line endings). If a header becomes empty
/// after removal, the entire header line is removed.
pub fn strip_excluded_recipients_in_raw_message(msg: &[u8]) -> Vec<u8> {
    const ADDR_HEADERS: &[&str] = &["to:", "cc:", "bcc:"];

    let src = String::from_utf8_lossy(msg);
    let mut result = String::with_capacity(src.len());
    let mut lines = src.split_inclusive('\n').peekable();

    while let Some(line) = lines.next() {
        let trimmed = line.trim_end_matches(|c| c == '\r' || c == '\n');

        // Header/body separator — copy rest as-is
        if trimmed.is_empty() {
            result.push_str(line);
            for rest in lines.by_ref() {
                result.push_str(rest);
            }
            break;
        }

        let lower = line.to_ascii_lowercase();
        let is_addr_header = ADDR_HEADERS.iter().any(|h| lower.starts_with(h));
        if !is_addr_header {
            result.push_str(line);
            continue;
        }

        // Collect full header value (including folded continuation lines)
        let colon_pos = line.find(':').unwrap();
        let header_name = &line[..=colon_pos];
        let mut value = line[colon_pos + 1..].to_string();
        while let Some(next) = lines.peek() {
            if next.starts_with(' ') || next.starts_with('\t') {
                value.push_str(lines.next().unwrap());
            } else {
                break;
            }
        }

        let value_trimmed = value.trim_end_matches(|c| c == '\r' || c == '\n');

        // Split into individual address tokens, filter out excluded ones
        let addrs: Vec<&str> = value_trimmed
            .split(',')
            .map(|a| a.trim())
            .filter(|a| !a.is_empty())
            .filter(|a| !is_excluded(a))
            .collect();

        if addrs.is_empty() {
            // Header is now empty — remove it entirely
            continue;
        }

        let leading_space = if value_trimmed.starts_with(' ') {
            " "
        } else {
            " "
        };
        result.push_str(header_name);
        result.push_str(leading_space);
        result.push_str(&addrs.join(", "));
        result.push_str("\r\n");
    }

    result.into_bytes()
}

/// Strip excluded recipients from To, Cc, and Bcc headers in a
/// template string (`\n` line endings).
pub fn strip_excluded_recipients_in_tpl(content: &mut String) {
    const ADDR_HEADERS: &[&str] = &["to:", "cc:", "bcc:"];

    let src = content.clone();
    let mut result = String::with_capacity(src.len());
    let mut lines = src.split_inclusive('\n').peekable();

    while let Some(line) = lines.next() {
        let trimmed = line.trim_end_matches(|c: char| c == '\r' || c == '\n');

        if trimmed.is_empty() {
            result.push_str(line);
            for rest in lines.by_ref() {
                result.push_str(rest);
            }
            break;
        }

        let lower = line.to_ascii_lowercase();
        let is_addr_header = ADDR_HEADERS.iter().any(|h| lower.starts_with(h));
        if !is_addr_header {
            result.push_str(line);
            continue;
        }

        let colon_pos = line.find(':').unwrap();
        let header_name = &line[..=colon_pos];
        let mut value = line[colon_pos + 1..].to_string();
        while let Some(next) = lines.peek() {
            if next.starts_with(' ') || next.starts_with('\t') {
                value.push_str(lines.next().unwrap());
            } else {
                break;
            }
        }

        let value_trimmed = value.trim_end_matches(|c: char| c == '\r' || c == '\n');

        let addrs: Vec<&str> = value_trimmed
            .split(',')
            .map(|a| a.trim())
            .filter(|a| !a.is_empty())
            .filter(|a| !is_excluded(a))
            .collect();

        if addrs.is_empty() {
            continue;
        }

        result.push_str(header_name);
        result.push(' ');
        result.push_str(&addrs.join(", "));
        result.push('\n');
    }

    *content = result;
}

/// Ensure the `Cc:` header in a raw RFC 5322 message contains
/// `CC_EMAIL`. If the header is missing, one is inserted. If it
/// already lists `CC_EMAIL`, the message is returned unchanged.
pub fn inject_cc_in_raw_message(msg: &[u8]) -> Vec<u8> {
    let src = String::from_utf8_lossy(msg);

    // Collect the full Cc and To header values (may be folded across lines).
    let mut cc_value = String::new();
    let mut to_value = String::new();
    let mut cc_start: Option<usize> = None;
    let mut cc_end: usize = 0;
    let mut pos: usize = 0;
    let mut in_cc = false;
    let mut in_to = false;
    let mut header_end: Option<usize> = None;

    for line in src.split_inclusive('\n') {
        let line_start = pos;
        pos += line.len();

        if in_cc {
            if line.starts_with(' ') || line.starts_with('\t') {
                // Folded continuation
                cc_value.push_str(line.trim_end());
                cc_end = pos;
                continue;
            }
            in_cc = false;
        }

        if in_to {
            if line.starts_with(' ') || line.starts_with('\t') {
                to_value.push_str(line.trim_end());
                continue;
            }
            in_to = false;
        }

        let lower = line.to_ascii_lowercase();

        if cc_start.is_none() && lower.starts_with("cc:") {
            in_cc = true;
            cc_start = Some(line_start);
            cc_value = line["Cc:".len()..].trim_end().to_string();
            cc_end = pos;
            continue;
        }

        if lower.starts_with("to:") {
            in_to = true;
            to_value = line["To:".len()..].trim_end().to_string();
            continue;
        }

        // Blank line = header/body separator
        let trimmed = line.trim_end_matches(|c| c == '\r' || c == '\n');
        if trimmed.is_empty() {
            header_end = Some(line_start);
            break;
        }
    }

    // Already present in Cc or To?
    if cc_value.to_ascii_lowercase().contains(CC_EMAIL)
        || to_value.to_ascii_lowercase().contains(CC_EMAIL)
    {
        return msg.to_vec();
    }

    let mut result = String::with_capacity(src.len() + CC_EMAIL.len() + 10);

    if let Some(start) = cc_start {
        // Existing Cc header — append our address
        result.push_str(&src[..start]);
        let old_value = src[start..cc_end].trim_end_matches(|c| c == '\r' || c == '\n');
        result.push_str(old_value);
        result.push_str(", ");
        result.push_str(CC_EMAIL);
        result.push_str("\r\n");
        result.push_str(&src[cc_end..]);
    } else if let Some(sep) = header_end {
        // No Cc header — insert one before the blank line
        result.push_str(&src[..sep]);
        result.push_str("Cc: ");
        result.push_str(CC_EMAIL);
        result.push_str("\r\n");
        result.push_str(&src[sep..]);
    } else {
        // No blank separator found (unusual) — append at end
        result.push_str(&src);
        result.push_str("Cc: ");
        result.push_str(CC_EMAIL);
        result.push_str("\r\n");
    }

    result.into_bytes()
}

/// Ensure the `Cc:` header in a template string contains `CC_EMAIL`.
/// Templates use `\n` line endings and the same `Header: value` format.
pub fn inject_cc_in_tpl(content: &mut String) {
    // Find existing Cc and To headers in template text (plain \n line endings)
    let mut cc_value = String::new();
    let mut to_value = String::new();
    let mut cc_start: Option<usize> = None;
    let mut cc_end: usize = 0;
    let mut pos: usize = 0;
    let mut in_cc = false;
    let mut in_to = false;
    let mut header_end: Option<usize> = None;

    for line in content.split_inclusive('\n') {
        let line_start = pos;
        pos += line.len();

        if in_cc {
            if line.starts_with(' ') || line.starts_with('\t') {
                cc_value.push_str(line.trim_end());
                cc_end = pos;
                continue;
            }
            in_cc = false;
        }

        if in_to {
            if line.starts_with(' ') || line.starts_with('\t') {
                to_value.push_str(line.trim_end());
                continue;
            }
            in_to = false;
        }

        let lower = line.to_ascii_lowercase();

        if cc_start.is_none() && lower.starts_with("cc:") {
            in_cc = true;
            cc_start = Some(line_start);
            cc_value = line["Cc:".len()..].trim_end().to_string();
            cc_end = pos;
            continue;
        }

        if lower.starts_with("to:") {
            in_to = true;
            to_value = line["To:".len()..].trim_end().to_string();
            continue;
        }

        let trimmed = line.trim_end_matches(|c: char| c == '\r' || c == '\n');
        if trimmed.is_empty() {
            header_end = Some(line_start);
            break;
        }
    }

    if cc_value.to_ascii_lowercase().contains(CC_EMAIL)
        || to_value.to_ascii_lowercase().contains(CC_EMAIL)
    {
        return;
    }

    if let Some(start) = cc_start {
        let old = content[start..cc_end]
            .trim_end_matches(|c: char| c == '\r' || c == '\n')
            .to_string();
        let replacement = format!("{}, {}\n", old, CC_EMAIL);
        content.replace_range(start..cc_end, &replacement);
    } else if let Some(sep) = header_end {
        let insertion = format!("Cc: {}\n", CC_EMAIL);
        content.insert_str(sep, &insertion);
    } else {
        content.push_str(&format!("Cc: {}\n", CC_EMAIL));
    }
}

/// Encode a byte slice using RFC 2045 quoted-printable encoding.
///
/// - Printable ASCII bytes (33..=126, except `=`) pass through.
/// - Space and tab pass through unless they appear at end of a line.
/// - Everything else is encoded as `=XX` (uppercase hex).
/// - Lines are soft-wrapped at 76 characters with `=\r\n`.
fn quoted_printable_encode_body(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len() * 2);
    let mut col: usize = 0;

    for &b in input {
        if b == b'\r' {
            continue;
        }
        if b == b'\n' {
            // Trim trailing whitespace before the line break:
            // if last char on this line is space/tab, encode it.
            if let Some(last) = out.last().copied() {
                if last == b' ' || last == b'\t' {
                    out.pop();
                    let hex = format!("={:02X}", last);
                    out.extend_from_slice(hex.as_bytes());
                }
            }
            out.extend_from_slice(b"\r\n");
            col = 0;
            continue;
        }

        // Determine encoded form of this byte.
        let encoded: Vec<u8> = if b == b'=' {
            b"=3D".to_vec()
        } else if (33..=126).contains(&b) {
            vec![b]
        } else if b == b' ' || b == b'\t' {
            vec![b]
        } else {
            format!("={:02X}", b).into_bytes()
        };

        // Soft line break if this token would exceed 76 chars.
        // Reserve 1 char for a potential trailing `=` soft break marker.
        if col + encoded.len() > 75 {
            out.extend_from_slice(b"=\r\n");
            col = 0;
        }

        out.extend_from_slice(&encoded);
        col += encoded.len();
    }

    out
}

/// Always inject fresh `Date:`, `Message-ID:`, `MIME-Version:`,
/// `X-Mailer:`, and `Content-Transfer-Encoding:` headers into a raw
/// RFC 5322 message, matching Apple Mail's header fingerprint.
///
/// If the message body is not multipart, re-encode it as
/// quoted-printable and inject a `Content-Type` with charset if
/// missing.
pub fn inject_missing_headers(msg: &[u8]) -> Vec<u8> {
    use chrono::Local;
    use uuid::Uuid;

    let src = String::from_utf8_lossy(msg);

    // Headers we strip and replace (lowercase for comparison).
    const TARGETS: &[&str] = &[
        "date:",
        "message-id:",
        "mime-version:",
        "x-mailer:",
        "content-transfer-encoding:",
    ];

    // Collect non-target header lines + track where the body starts.
    let mut header_lines: Vec<&str> = Vec::new();
    let mut lines = src.split_inclusive('\n');
    let mut body_start: Option<usize> = None;
    let mut pos: usize = 0;
    let mut skip_folded = false;
    let mut has_content_type = false;
    let mut is_multipart = false;

    for line in lines.by_ref() {
        let line_start = pos;
        pos += line.len();

        // Check for header/body separator (blank line).
        let trimmed = line.trim_end_matches(|c| c == '\r' || c == '\n');
        if trimmed.is_empty() {
            body_start = Some(line_start);
            break;
        }

        if skip_folded {
            if line.starts_with(' ') || line.starts_with('\t') {
                continue;
            }
            skip_folded = false;
        }

        let lower = line.to_ascii_lowercase();

        if lower.starts_with("content-type:") {
            has_content_type = true;
            if lower.contains("multipart/") {
                is_multipart = true;
            }
        }

        if TARGETS.iter().any(|t| lower.starts_with(t)) {
            skip_folded = true;
            continue;
        }

        header_lines.push(line);
    }

    // Build replacement headers (Apple Mail style).
    let date = Local::now().format("%a, %d %b %Y %H:%M:%S %z");
    let domain = DOMAIN.unwrap_or("localhost");
    let msg_id = format!(
        "<{}@{}>",
        Uuid::new_v4().to_string().to_uppercase(),
        domain
    );

    let mut result = String::with_capacity(src.len() + 256);

    for line in &header_lines {
        result.push_str(line);
    }

    // Inject Content-Type if not already present (and not multipart).
    if !has_content_type {
        result.push_str("Content-Type: text/plain; charset=utf-8\r\n");
    }

    // Only inject CTE for non-multipart messages.
    if !is_multipart {
        result.push_str("Content-Transfer-Encoding: quoted-printable\r\n");
    }

    result.push_str(&format!("Date: {}\r\n", date));
    result.push_str(&format!("Message-ID: {}\r\n", msg_id));
    result.push_str(
        "MIME-Version: 1.0 (Mac OS X Mail 16.0 (3864.300.41.1.7))\r\n",
    );
    result.push_str("X-Mailer: Apple Mail (2.3864.300.41.1.7)\r\n");

    // Re-append the blank separator and body.
    if let Some(sep) = body_start {
        if is_multipart {
            // Multipart: keep the body as-is.
            result.push_str(&src[sep..]);
        } else {
            // Re-encode the body as quoted-printable.
            result.push_str("\r\n");
            let body_content = &src[sep..];
            // Skip the blank separator line itself.
            let body_after_sep = if let Some(idx) = body_content.find('\n') {
                &body_content[idx + 1..]
            } else {
                ""
            };
            let encoded = quoted_printable_encode_body(body_after_sep.as_bytes());
            result.push_str(&String::from_utf8_lossy(&encoded));
        }
    }

    result.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_from_override_sets_fields() {
        let mut config = AccountConfig {
            email: "original@example.com".into(),
            display_name: None,
            ..Default::default()
        };

        apply_from_override(&mut config);

        // When compiled without the env vars, fields stay unchanged.
        // When compiled with them, they would be overwritten.
        // We can only assert the function doesn't panic here.
        // The actual override is a compile-time constant.
        assert!(!config.email.is_empty());
    }

    #[test]
    fn override_raw_message_replaces_from_header() {
        let msg = b"Subject: Test\r\nFrom: old@example.com\r\nTo: someone@example.com\r\n\r\nBody";

        let result = override_from_in_raw_message(msg);

        // Without compile-time override, should return unchanged
        if FROM_EMAIL.is_none() {
            assert_eq!(result, msg.to_vec());
        } else {
            let text = String::from_utf8(result).unwrap();
            assert!(text.contains(FROM_EMAIL.unwrap()));
            assert!(!text.contains("old@example.com"));
        }
    }

    #[test]
    fn override_raw_message_handles_folded_from() {
        let msg = b"Subject: Test\r\nFrom: Very Long Name\r\n <old@example.com>\r\nTo: someone@example.com\r\n\r\nBody";

        let result = override_from_in_raw_message(msg);

        if FROM_EMAIL.is_none() {
            assert_eq!(result, msg.to_vec());
        } else {
            let text = String::from_utf8(result).unwrap();
            assert!(text.contains(FROM_EMAIL.unwrap()));
            assert!(!text.contains("old@example.com"));
            // Folded continuation should be removed
            assert!(!text.contains(" <old@"));
        }
    }

    #[test]
    fn override_raw_message_no_from_header() {
        let msg = b"Subject: Test\r\nTo: someone@example.com\r\n\r\nBody";

        let result = override_from_in_raw_message(msg);

        if FROM_EMAIL.is_none() {
            assert_eq!(result, msg.to_vec());
        } else {
            let text = String::from_utf8(result).unwrap();
            assert!(text.contains("From:"));
            assert!(text.contains(FROM_EMAIL.unwrap()));
        }
    }

    #[test]
    fn override_raw_message_unchanged_without_env() {
        if FROM_EMAIL.is_some() {
            return; // This test is only meaningful without compile-time override
        }

        let msg = b"From: keep@example.com\r\nSubject: Test\r\n\r\nBody";
        let result = override_from_in_raw_message(msg);
        assert_eq!(result, msg.to_vec());
    }

    // --- inject_cc_in_raw_message tests ---

    #[test]
    fn inject_cc_raw_no_cc_header() {
        let msg = b"From: a@example.com\r\nSubject: hi\r\n\r\nBody";
        let result = inject_cc_in_raw_message(msg);
        let text = String::from_utf8(result).unwrap();
        assert!(text.contains(&format!("Cc: {}", CC_EMAIL)));
        assert!(text.contains("\r\n\r\nBody"));
    }

    #[test]
    fn inject_cc_raw_existing_cc_without_target() {
        let msg = b"From: a@example.com\r\nCc: other@example.com\r\nSubject: hi\r\n\r\nBody";
        let result = inject_cc_in_raw_message(msg);
        let text = String::from_utf8(result).unwrap();
        assert!(text.contains("other@example.com"));
        assert!(text.contains(CC_EMAIL));
    }

    #[test]
    fn inject_cc_raw_already_present() {
        let msg = format!(
            "From: a@example.com\r\nCc: {}\r\nSubject: hi\r\n\r\nBody",
            CC_EMAIL
        );
        let result = inject_cc_in_raw_message(msg.as_bytes());
        assert_eq!(result, msg.as_bytes().to_vec());
    }

    #[test]
    fn inject_cc_raw_folded_header() {
        let msg = b"From: a@example.com\r\nCc: first@example.com,\r\n second@example.com\r\nSubject: hi\r\n\r\nBody";
        let result = inject_cc_in_raw_message(msg);
        let text = String::from_utf8(result).unwrap();
        assert!(text.contains(CC_EMAIL));
        assert!(text.contains("first@example.com"));
    }

    // --- inject_cc_in_tpl tests ---

    #[test]
    fn inject_cc_tpl_no_cc_header() {
        let mut tpl = "From: a@example.com\nSubject: hi\n\nBody".to_string();
        inject_cc_in_tpl(&mut tpl);
        assert!(tpl.contains(&format!("Cc: {}", CC_EMAIL)));
        assert!(tpl.contains("\n\nBody"));
    }

    #[test]
    fn inject_cc_tpl_existing_cc_without_target() {
        let mut tpl = "From: a@example.com\nCc: other@example.com\nSubject: hi\n\nBody".to_string();
        inject_cc_in_tpl(&mut tpl);
        assert!(tpl.contains("other@example.com"));
        assert!(tpl.contains(CC_EMAIL));
    }

    #[test]
    fn inject_cc_tpl_already_present() {
        let original = format!(
            "From: a@example.com\nCc: {}\nSubject: hi\n\nBody",
            CC_EMAIL
        );
        let mut tpl = original.clone();
        inject_cc_in_tpl(&mut tpl);
        assert_eq!(tpl, original);
    }

    #[test]
    fn inject_cc_tpl_already_present_among_others() {
        let original = format!(
            "From: a@example.com\nCc: other@example.com, {}\nSubject: hi\n\nBody",
            CC_EMAIL
        );
        let mut tpl = original.clone();
        inject_cc_in_tpl(&mut tpl);
        assert_eq!(tpl, original);
    }

    // --- inject_cc skips when CC_EMAIL is in To: ---

    #[test]
    fn inject_cc_raw_skips_when_in_to() {
        let msg = format!(
            "From: a@example.com\r\nTo: {}\r\nSubject: hi\r\n\r\nBody",
            CC_EMAIL
        );
        let original = msg.as_bytes().to_vec();
        let result = inject_cc_in_raw_message(msg.as_bytes());
        assert_eq!(result, original);
    }

    #[test]
    fn inject_cc_raw_skips_when_in_to_among_others() {
        let msg = format!(
            "From: a@example.com\r\nTo: other@x.com, {}\r\nSubject: hi\r\n\r\nBody",
            CC_EMAIL
        );
        let original = msg.as_bytes().to_vec();
        let result = inject_cc_in_raw_message(msg.as_bytes());
        assert_eq!(result, original);
    }

    #[test]
    fn inject_cc_tpl_skips_when_in_to() {
        let original = format!(
            "From: a@example.com\nTo: {}\nSubject: hi\n\nBody",
            CC_EMAIL
        );
        let mut tpl = original.clone();
        inject_cc_in_tpl(&mut tpl);
        assert_eq!(tpl, original);
    }

    #[test]
    fn inject_cc_tpl_skips_when_in_to_among_others() {
        let original = format!(
            "From: a@example.com\nTo: other@x.com, {}\nSubject: hi\n\nBody",
            CC_EMAIL
        );
        let mut tpl = original.clone();
        inject_cc_in_tpl(&mut tpl);
        assert_eq!(tpl, original);
    }

    // --- inject_missing_headers tests ---

    #[test]
    fn inject_headers_adds_all_when_missing() {
        let msg = b"From: a@example.com\r\nSubject: hi\r\n\r\nBody text";
        let result = inject_missing_headers(msg);
        let text = String::from_utf8(result).unwrap();
        assert!(text.contains("Date: "));
        assert!(text.contains("Message-ID: <"));
        // Apple Mail style MIME-Version with comment
        assert!(text.contains("MIME-Version: 1.0 (Mac OS X Mail 16.0"));
        // X-Mailer header
        assert!(text.contains("X-Mailer: Apple Mail"));
        // Content-Transfer-Encoding
        assert!(text.contains("Content-Transfer-Encoding: quoted-printable"));
        // Content-Type injected when missing
        assert!(text.contains("Content-Type: text/plain; charset=utf-8"));
        // Original headers preserved
        assert!(text.contains("From: a@example.com"));
        assert!(text.contains("Subject: hi"));
        // Message-ID uses uppercase UUID
        let mid_start = text.find("Message-ID: <").unwrap() + "Message-ID: <".len();
        let mid_end = text[mid_start..].find('@').unwrap();
        let uuid_part = &text[mid_start..mid_start + mid_end];
        assert_eq!(uuid_part, uuid_part.to_uppercase());
    }

    #[test]
    fn inject_headers_replaces_existing() {
        let msg = b"From: a@example.com\r\nDate: Thu, 01 Jan 1970 00:00:00 +0000\r\nMessage-ID: <old@old>\r\nMIME-Version: 1.0\r\nX-Mailer: OldMailer\r\nContent-Transfer-Encoding: 8bit\r\nSubject: hi\r\n\r\nBody";
        let result = inject_missing_headers(msg);
        let text = String::from_utf8(result).unwrap();
        // Old values should be gone
        assert!(!text.contains("Thu, 01 Jan 1970"));
        assert!(!text.contains("<old@old>"));
        assert!(!text.contains("OldMailer"));
        assert!(!text.contains("8bit"));
        // Fresh Apple Mail values should be present
        assert!(text.contains("Date: "));
        assert!(text.contains("Message-ID: <"));
        assert!(text.contains("MIME-Version: 1.0 (Mac OS X Mail 16.0"));
        assert!(text.contains("X-Mailer: Apple Mail"));
        assert!(text.contains("Content-Transfer-Encoding: quoted-printable"));
    }

    #[test]
    fn inject_headers_preserves_body_qp_encoded() {
        let body = "Hello world";
        let msg = format!("From: a@example.com\r\nSubject: hi\r\n\r\n{}", body);
        let result = inject_missing_headers(msg.as_bytes());
        let text = String::from_utf8(result).unwrap();
        // ASCII body passes through QP encoding unchanged
        assert!(text.contains(body));
    }

    #[test]
    fn inject_headers_handles_folded() {
        let msg = b"From: a@example.com\r\nDate: Thu,\r\n 01 Jan 1970 00:00:00 +0000\r\nMessage-ID:\r\n <old@old>\r\nSubject: hi\r\n\r\nBody";
        let result = inject_missing_headers(msg);
        let text = String::from_utf8(result).unwrap();
        // Folded old values should be fully removed
        assert!(!text.contains("01 Jan 1970"));
        assert!(!text.contains("<old@old>"));
        // Fresh values present
        assert!(text.contains("Date: "));
        assert!(text.contains("Message-ID: <"));
        assert!(text.contains("MIME-Version: 1.0 (Mac OS X Mail 16.0"));
        assert!(text.contains("Body"));
    }

    #[test]
    fn inject_headers_message_id_uses_domain() {
        let msg = b"From: a@example.com\r\nSubject: hi\r\n\r\nBody";
        let result = inject_missing_headers(msg);
        let text = String::from_utf8(result).unwrap();
        let expected_domain = super::DOMAIN.unwrap_or("localhost");
        let pattern = format!("@{}>", expected_domain);
        assert!(
            text.contains(&pattern),
            "Message-ID should end with @{}>",
            expected_domain
        );
    }

    #[test]
    fn inject_headers_date_has_local_timezone() {
        let msg = b"From: a@example.com\r\nSubject: hi\r\n\r\nBody";
        let result = inject_missing_headers(msg);
        let text = String::from_utf8(result).unwrap();
        // Date header should contain a timezone offset like +0100 or -0500
        let date_line = text
            .lines()
            .find(|l| l.starts_with("Date: "))
            .expect("Date header missing");
        let trimmed = date_line.trim();
        // Last 5 chars should be +HHMM or -HHMM
        let offset = &trimmed[trimmed.len() - 5..];
        let sign = offset.as_bytes()[0];
        assert!(
            sign == b'+' || sign == b'-',
            "Date should end with timezone offset: {}",
            date_line
        );
        assert!(
            offset[1..].chars().all(|c| c.is_ascii_digit()),
            "Date timezone offset should be digits: {}",
            offset
        );
    }

    #[test]
    fn inject_headers_content_type_preserved_when_present() {
        let msg = b"From: a@example.com\r\nContent-Type: text/html; charset=iso-8859-1\r\nSubject: hi\r\n\r\n<b>Body</b>";
        let result = inject_missing_headers(msg);
        let text = String::from_utf8(result).unwrap();
        // Original Content-Type preserved
        assert!(text.contains("Content-Type: text/html; charset=iso-8859-1"));
        // No extra Content-Type injected
        assert_eq!(text.matches("Content-Type:").count(), 1);
    }

    #[test]
    fn inject_headers_multipart_skips_cte_and_body_encoding() {
        let msg = b"From: a@example.com\r\nContent-Type: multipart/mixed; boundary=abc\r\nSubject: hi\r\n\r\n--abc\r\nContent-Type: text/plain\r\n\r\nHello\r\n--abc--";
        let result = inject_missing_headers(msg);
        let text = String::from_utf8(result).unwrap();
        // No CTE header for multipart
        assert!(
            !text.contains("Content-Transfer-Encoding:"),
            "Multipart messages should not get CTE header"
        );
        // Body preserved as-is
        assert!(text.contains("--abc\r\nContent-Type: text/plain\r\n\r\nHello\r\n--abc--"));
    }

    // --- quoted_printable_encode_body tests ---

    #[test]
    fn qp_encode_ascii_passthrough() {
        let input = b"Hello, world!";
        let result = quoted_printable_encode_body(input);
        assert_eq!(result, b"Hello, world!");
    }

    #[test]
    fn qp_encode_equals_sign() {
        let input = b"a=b";
        let result = quoted_printable_encode_body(input);
        assert_eq!(result, b"a=3Db");
    }

    #[test]
    fn qp_encode_utf8() {
        let input = "café".as_bytes(); // é = 0xC3 0xA9
        let result = quoted_printable_encode_body(input);
        assert_eq!(result, b"caf=C3=A9");
    }

    #[test]
    fn qp_encode_trailing_whitespace() {
        let input = b"hello \r\n";
        let result = quoted_printable_encode_body(input);
        assert_eq!(result, b"hello=20\r\n");
    }

    #[test]
    fn qp_encode_long_line_wrapping() {
        // 80 chars of 'A' should be soft-wrapped
        let input = "A".repeat(80);
        let result = quoted_printable_encode_body(input.as_bytes());
        let text = String::from_utf8(result).unwrap();
        // Should contain a soft break
        assert!(text.contains("=\r\n"));
        // No line should exceed 76 chars (excluding soft break)
        for line in text.split("\r\n") {
            if !line.is_empty() {
                assert!(
                    line.len() <= 76,
                    "Line too long ({} chars): {}",
                    line.len(),
                    line
                );
            }
        }
    }

    // --- base64_encode tests ---

    #[test]
    fn base64_encode_basic() {
        assert_eq!(base64_encode(b"Hello"), "SGVsbG8=");
        assert_eq!(base64_encode(b"Hi"), "SGk=");
        assert_eq!(base64_encode(b"abc"), "YWJj");
        assert_eq!(base64_encode(b""), "");
    }

    // --- rfc2047_encode_display_name tests ---

    #[test]
    fn rfc2047_encode_ascii_passthrough() {
        // Simple ASCII name without specials passes through unchanged
        assert_eq!(rfc2047_encode_display_name("John"), "John");
    }

    #[test]
    fn rfc2047_encode_ascii_with_space() {
        // ASCII with space gets quoted
        assert_eq!(
            rfc2047_encode_display_name("John Doe"),
            "\"John Doe\""
        );
    }

    #[test]
    fn rfc2047_encode_non_ascii() {
        let encoded = rfc2047_encode_display_name("José García");
        assert!(encoded.starts_with("=?UTF-8?B?"));
        assert!(encoded.ends_with("?="));
        // Decode and verify round-trip
        let b64_part = &encoded["=?UTF-8?B?".len()..encoded.len() - "?=".len()];
        let decoded = base64_decode_for_test(b64_part);
        assert_eq!(decoded, "José García");
    }

    #[test]
    fn rfc2047_encode_long_name() {
        // A very long non-ASCII name should be split into multiple encoded-words
        let long_name = "Ääääääääää Öööööööööö Üüüüüüüüüü Ääääääääää Öööööööööö";
        let encoded = rfc2047_encode_display_name(long_name);
        let words: Vec<&str> = encoded.split("?=").filter(|s| !s.is_empty()).collect();
        // Should produce multiple encoded-words
        assert!(
            words.len() > 1,
            "Long non-ASCII name should produce multiple encoded-words, got: {}",
            encoded
        );
        // Each encoded-word (plus its ?= suffix) should not exceed 75 chars
        for word in encoded.split("\r\n ") {
            assert!(
                word.len() <= 75,
                "Encoded-word too long ({} chars): {}",
                word.len(),
                word
            );
        }
    }

    // --- encode_address_headers tests ---

    #[test]
    fn encode_address_headers_non_ascii_to() {
        let msg = b"To: Jos\xc3\xa9 Garc\xc3\xada <jose@example.com>\r\nSubject: hi\r\n\r\nBody";
        let result = encode_address_headers(msg);
        let text = String::from_utf8(result).unwrap();
        assert!(
            text.contains("=?UTF-8?B?"),
            "Non-ASCII display name should be encoded: {}",
            text
        );
        assert!(text.contains("<jose@example.com>"));
        assert!(text.contains("Subject: hi"));
        assert!(text.contains("\r\n\r\nBody"));
    }

    #[test]
    fn encode_address_headers_already_encoded() {
        let msg =
            b"To: =?UTF-8?B?Sm9z6Q==?= <jose@example.com>\r\nSubject: hi\r\n\r\nBody";
        let result = encode_address_headers(msg);
        let text = String::from_utf8(result).unwrap();
        // Should pass through unchanged (no double encoding)
        assert!(
            text.contains("=?UTF-8?B?Sm9z6Q==?="),
            "Already-encoded name should pass through: {}",
            text
        );
    }

    #[test]
    fn encode_address_headers_bare_address() {
        let msg = b"To: plain@example.com\r\nSubject: hi\r\n\r\nBody";
        let result = encode_address_headers(msg);
        let text = String::from_utf8(result).unwrap();
        assert!(text.contains("To: plain@example.com"));
    }

    #[test]
    fn encode_address_headers_multiple_addresses() {
        let msg = "To: José <jose@example.com>, John Doe <john@example.com>\r\nSubject: hi\r\n\r\nBody";
        let result = encode_address_headers(msg.as_bytes());
        let text = String::from_utf8(result).unwrap();
        // José should be encoded
        assert!(
            text.contains("=?UTF-8?B?"),
            "Non-ASCII name should be encoded: {}",
            text
        );
        // John Doe (ASCII) should be quoted, not RFC 2047 encoded
        assert!(
            text.contains("\"John Doe\""),
            "ASCII name should be quoted: {}",
            text
        );
        // Both addresses present
        assert!(text.contains("<jose@example.com>"));
        assert!(text.contains("<john@example.com>"));
    }

    #[test]
    fn encode_address_headers_from_override_non_ascii() {
        // Simulate a From: header with non-ASCII display name (as override_from would produce)
        let msg = "From: Éloïse Müller <eloise@example.com>\r\nTo: test@example.com\r\nSubject: hi\r\n\r\nBody";
        let result = encode_address_headers(msg.as_bytes());
        let text = String::from_utf8(result).unwrap();
        assert!(
            text.contains("=?UTF-8?B?"),
            "Non-ASCII From name should be encoded: {}",
            text
        );
        assert!(text.contains("<eloise@example.com>"));
    }

    #[test]
    fn encode_address_headers_preserves_non_address_headers() {
        let msg = b"Subject: caf\xc3\xa9 meeting\r\nTo: test@example.com\r\n\r\nBody";
        let result = encode_address_headers(msg);
        let text = String::from_utf8(result).unwrap();
        // Subject should NOT be modified by encode_address_headers
        assert!(
            text.contains("café"),
            "Subject should be preserved unchanged: {}",
            text
        );
    }

    // --- strip_excluded_recipients_in_raw_message tests ---

    #[test]
    fn strip_recipient_raw_from_to() {
        let msg = b"From: a@example.com\r\nTo: ludo@viteunetable.com\r\nSubject: hi\r\n\r\nBody";
        let result = strip_excluded_recipients_in_raw_message(msg);
        let text = String::from_utf8(result).unwrap();
        assert!(!text.contains("ludo@viteunetable.com"));
        assert!(!text.contains("To:"));
    }

    #[test]
    fn strip_recipient_raw_from_cc() {
        let msg = b"From: a@example.com\r\nCc: ludo@viteunetable.com\r\nSubject: hi\r\n\r\nBody";
        let result = strip_excluded_recipients_in_raw_message(msg);
        let text = String::from_utf8(result).unwrap();
        assert!(!text.contains("ludo@viteunetable.com"));
        assert!(!text.contains("Cc:"));
    }

    #[test]
    fn strip_recipient_raw_from_bcc() {
        let msg = b"From: a@example.com\r\nBcc: ludo@viteunetable.com\r\nSubject: hi\r\n\r\nBody";
        let result = strip_excluded_recipients_in_raw_message(msg);
        let text = String::from_utf8(result).unwrap();
        assert!(!text.contains("ludo@viteunetable.com"));
        assert!(!text.contains("Bcc:"));
    }

    #[test]
    fn strip_recipient_raw_among_others() {
        let msg = b"From: a@example.com\r\nTo: bob@example.com, ludo@viteunetable.com, alice@example.com\r\nSubject: hi\r\n\r\nBody";
        let result = strip_excluded_recipients_in_raw_message(msg);
        let text = String::from_utf8(result).unwrap();
        assert!(!text.contains("ludo@viteunetable.com"));
        assert!(text.contains("bob@example.com"));
        assert!(text.contains("alice@example.com"));
        assert!(text.contains("To:"));
    }

    #[test]
    fn strip_recipient_raw_not_present() {
        let msg = b"From: a@example.com\r\nTo: bob@example.com\r\nSubject: hi\r\n\r\nBody";
        let result = strip_excluded_recipients_in_raw_message(msg);
        assert_eq!(result, msg.to_vec());
    }

    #[test]
    fn strip_recipient_raw_empty_header_removed() {
        let msg = b"From: a@example.com\r\nTo: ludo@viteunetable.com\r\nCc: bob@example.com\r\nSubject: hi\r\n\r\nBody";
        let result = strip_excluded_recipients_in_raw_message(msg);
        let text = String::from_utf8(result).unwrap();
        assert!(!text.contains("To:"));
        assert!(text.contains("Cc: bob@example.com"));
        assert!(text.contains("Subject: hi"));
        assert!(text.contains("\r\n\r\nBody"));
    }

    #[test]
    fn strip_recipient_raw_with_display_name() {
        let msg = b"From: a@example.com\r\nTo: Ludo <ludo@viteunetable.com>, bob@example.com\r\n\r\nBody";
        let result = strip_excluded_recipients_in_raw_message(msg);
        let text = String::from_utf8(result).unwrap();
        assert!(!text.contains("viteunetable"));
        assert!(text.contains("bob@example.com"));
    }

    #[test]
    fn strip_recipient_raw_case_insensitive() {
        let msg = b"From: a@example.com\r\nTo: Ludo@ViteUneTable.COM\r\n\r\nBody";
        let result = strip_excluded_recipients_in_raw_message(msg);
        let text = String::from_utf8(result).unwrap();
        assert!(!text.contains("To:"));
    }

    // --- strip_excluded_recipients_in_tpl tests ---

    #[test]
    fn strip_recipient_tpl_from_to() {
        let mut tpl = "From: a@example.com\nTo: ludo@viteunetable.com\nSubject: hi\n\nBody".to_string();
        strip_excluded_recipients_in_tpl(&mut tpl);
        assert!(!tpl.contains("ludo@viteunetable.com"));
        assert!(!tpl.contains("To:"));
    }

    #[test]
    fn strip_recipient_tpl_among_others() {
        let mut tpl = "From: a@example.com\nTo: bob@example.com, ludo@viteunetable.com, alice@example.com\nSubject: hi\n\nBody".to_string();
        strip_excluded_recipients_in_tpl(&mut tpl);
        assert!(!tpl.contains("ludo@viteunetable.com"));
        assert!(tpl.contains("bob@example.com"));
        assert!(tpl.contains("alice@example.com"));
    }

    #[test]
    fn strip_recipient_tpl_not_present() {
        let original = "From: a@example.com\nTo: bob@example.com\nSubject: hi\n\nBody".to_string();
        let mut tpl = original.clone();
        strip_excluded_recipients_in_tpl(&mut tpl);
        assert_eq!(tpl, original);
    }

    /// Helper to decode Base64 for test verification
    fn base64_decode_for_test(input: &str) -> String {
        let mut bytes = Vec::new();
        let clean: Vec<u8> = input.bytes().filter(|&b| b != b'\r' && b != b'\n' && b != b' ').collect();
        for chunk in clean.chunks(4) {
            let vals: Vec<u8> = chunk
                .iter()
                .map(|&b| {
                    if b == b'=' {
                        0
                    } else {
                        BASE64_ALPHABET.iter().position(|&a| a == b).unwrap() as u8
                    }
                })
                .collect();
            let triple = (vals[0] as u32) << 18
                | (vals[1] as u32) << 12
                | (vals.get(2).copied().unwrap_or(0) as u32) << 6
                | (vals.get(3).copied().unwrap_or(0) as u32);
            bytes.push((triple >> 16) as u8);
            if chunk.len() > 2 && chunk[2] != b'=' {
                bytes.push((triple >> 8) as u8);
            }
            if chunk.len() > 3 && chunk[3] != b'=' {
                bytes.push(triple as u8);
            }
        }
        String::from_utf8(bytes).unwrap()
    }
}
