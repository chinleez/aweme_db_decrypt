//! `watch` subcommand: tail new IM messages from the live encrypted database.

use crate::db::open::open_encrypted_direct;

use anyhow::Result;
use clap::{Args, ValueEnum};
use rusqlite::{params, Connection, Row};
use serde_json::Value;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum WatchOutput {
    Chat,
    Jsonl,
}

#[derive(Args, Debug)]
pub struct WatchArgs {
    /// Path to encrypted_<uid>_im.db.
    pub input: PathBuf,

    /// Optional path to encrypted_im_biz_<uid>.db for user nickname lookup.
    #[arg(long = "biz-db", value_name = "PATH")]
    pub biz_db: Option<PathBuf>,

    /// Optional path to encrypted_<uid>_im_fts_split.db for search-content fallback.
    #[arg(long = "fts-db", value_name = "PATH")]
    pub fts_db: Option<PathBuf>,

    /// User UID (numeric). Defaults to the value parsed from the filename.
    #[arg(short, long)]
    pub uid: Option<String>,

    /// Poll interval in milliseconds.
    #[arg(short = 'i', long, default_value_t = 1000)]
    pub interval_ms: u64,

    /// Print the latest N historical messages first, then continue watching.
    #[arg(long, default_value_t = 0)]
    pub recent: usize,

    /// Start at current tail and only print future messages. This is the default.
    #[arg(long)]
    pub from_now: bool,

    /// Start from the beginning of the msg table. Usually noisy; prefer --recent.
    #[arg(long, conflicts_with = "from_now")]
    pub from_beginning: bool,

    /// Only watch one conversation_id.
    #[arg(long)]
    pub conversation_id: Option<String>,

    /// Stop after one polling iteration.
    #[arg(long)]
    pub once: bool,

    /// Output format.
    #[arg(long, value_enum, default_value_t = WatchOutput::Chat)]
    pub output: WatchOutput,

    /// Print at most this many new messages per polling iteration; 0 means no limit.
    #[arg(long, default_value_t = 0)]
    pub limit: usize,

    /// POST new messages to an HTTP endpoint, for example http://host:8787/api/messages.
    #[arg(long = "post-url", value_name = "URL")]
    pub post_url: Option<String>,

    /// Also POST messages printed by --recent. By default only future messages are posted.
    #[arg(long)]
    pub post_recent: bool,

    /// HTTP POST timeout in milliseconds.
    #[arg(long, default_value_t = 3000)]
    pub post_timeout_ms: u64,
}

#[derive(Debug, Clone)]
struct Cursor {
    created_time: i64,
    msg_uuid: String,
}

#[derive(Debug, Clone)]
struct Message {
    msg_uuid: String,
    msg_server_id: Option<i64>,
    conversation_id: String,
    conversation_name: String,
    conversation_type: Option<i64>,
    created_time: i64,
    time_text: String,
    sender: Option<i64>,
    sender_name: String,
    msg_type: Option<i64>,
    content: String,
    search_content: Option<String>,
}

#[derive(Debug, Clone)]
struct HttpEndpoint {
    host: String,
    port: u16,
    path: String,
    host_header: String,
}

pub fn run(args: WatchArgs) -> Result<()> {
    let opened = open_encrypted_direct(&args.input, args.uid.as_deref())?;
    eprintln!(
        "[+] opened: {} uid={} mode=live-readonly",
        opened.kind.label(),
        opened.uid
    );

    let biz = if let Some(path) = &args.biz_db {
        let biz = open_encrypted_direct(path, args.uid.as_deref())?;
        eprintln!("[+] attached biz: {}", path.display());
        Some(biz)
    } else {
        None
    };
    let biz_conn = biz.as_ref().map(|db| &db.conn);
    let self_uid = opened.uid.parse::<i64>().ok();

    let fts = if let Some(path) = &args.fts_db {
        let fts = open_encrypted_direct(path, Some(&opened.uid))?;
        eprintln!("[+] attached fts: {}", path.display());
        Some(fts)
    } else {
        None
    };
    let fts_conn = fts.as_ref().map(|db| &db.conn);

    let mut cursor = if args.from_beginning {
        Cursor { created_time: -1, msg_uuid: String::new() }
    } else {
        tail_cursor(&opened.conn, args.conversation_id.as_deref())?
    };

    if args.recent > 0 {
        let messages = fetch_recent(&opened.conn, biz_conn, fts_conn, self_uid, &args, args.recent)?;
        for message in &messages {
            print_message(message, args.output)?;
        }
        if args.post_recent {
            post_messages_if_needed(&args, &opened.uid, &messages);
        }
        if let Some(last) = messages.last() {
            cursor = Cursor {
                created_time: last.created_time,
                msg_uuid: last.msg_uuid.clone(),
            };
        } else {
            cursor = tail_cursor(&opened.conn, args.conversation_id.as_deref())?;
        }
        io::stdout().flush().ok();
    }

    loop {
        let messages = fetch_after(&opened.conn, biz_conn, fts_conn, self_uid, &args, &cursor)?;
        for message in &messages {
            print_message(message, args.output)?;
        }
        post_messages_if_needed(&args, &opened.uid, &messages);
        if let Some(last) = messages.last() {
            cursor = Cursor {
                created_time: last.created_time,
                msg_uuid: last.msg_uuid.clone(),
            };
        }
        io::stdout().flush().ok();

        if args.once {
            break;
        }
        thread::sleep(Duration::from_millis(args.interval_ms));
    }

    Ok(())
}

fn tail_cursor(conn: &Connection, conversation_id: Option<&str>) -> Result<Cursor> {
    let row = if let Some(conversation_id) = conversation_id {
        conn.query_row(
            "SELECT created_time, msg_uuid FROM msg WHERE conversation_id = ? ORDER BY created_time DESC, msg_uuid DESC LIMIT 1",
            [conversation_id],
            |r| Ok(Cursor { created_time: r.get(0)?, msg_uuid: r.get(1)? }),
        )
        .optional()?
    } else {
        conn.query_row(
            "SELECT created_time, msg_uuid FROM msg ORDER BY created_time DESC, msg_uuid DESC LIMIT 1",
            [],
            |r| Ok(Cursor { created_time: r.get(0)?, msg_uuid: r.get(1)? }),
        )
        .optional()?
    };
    Ok(row.unwrap_or(Cursor { created_time: -1, msg_uuid: String::new() }))
}

fn fetch_recent(
    conn: &Connection,
    biz_conn: Option<&Connection>,
    fts_conn: Option<&Connection>,
    self_uid: Option<i64>,
    args: &WatchArgs,
    limit: usize,
) -> Result<Vec<Message>> {
    let sql = select_sql(args.conversation_id.is_some(), true, args.limit);
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = if let Some(conversation_id) = &args.conversation_id {
        stmt.query_map(params![conversation_id, limit as i64], row_to_message)?
            .collect::<std::result::Result<Vec<_>, _>>()?
    } else {
        stmt.query_map(params![limit as i64], row_to_message)?
            .collect::<std::result::Result<Vec<_>, _>>()?
    };
    hydrate_names(conn, biz_conn, self_uid, &mut rows);
    hydrate_search_content(fts_conn, &mut rows);
    Ok(rows)
}

fn fetch_after(
    conn: &Connection,
    biz_conn: Option<&Connection>,
    fts_conn: Option<&Connection>,
    self_uid: Option<i64>,
    args: &WatchArgs,
    cursor: &Cursor,
) -> Result<Vec<Message>> {
    let sql = select_sql(args.conversation_id.is_some(), false, args.limit);
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = if let Some(conversation_id) = &args.conversation_id {
        if args.limit > 0 {
            stmt.query_map(
                params![
                    cursor.created_time,
                    cursor.created_time,
                    cursor.msg_uuid,
                    conversation_id,
                    args.limit as i64
                ],
                row_to_message,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(
                params![
                    cursor.created_time,
                    cursor.created_time,
                    cursor.msg_uuid,
                    conversation_id
                ],
                row_to_message,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?
        }
    } else if args.limit > 0 {
        stmt.query_map(
            params![cursor.created_time, cursor.created_time, cursor.msg_uuid, args.limit as i64],
            row_to_message,
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?
    } else {
        stmt.query_map(
            params![cursor.created_time, cursor.created_time, cursor.msg_uuid],
            row_to_message,
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?
    };
    hydrate_names(conn, biz_conn, self_uid, &mut rows);
    hydrate_search_content(fts_conn, &mut rows);
    Ok(rows)
}

fn select_sql(has_conversation_filter: bool, recent: bool, limit: usize) -> String {
    let where_clause = if recent {
        if has_conversation_filter {
            "WHERE m.conversation_id = ?"
        } else {
            ""
        }
    } else if has_conversation_filter {
        "WHERE (m.created_time > ? OR (m.created_time = ? AND m.msg_uuid > ?)) AND m.conversation_id = ?"
    } else {
        "WHERE m.created_time > ? OR (m.created_time = ? AND m.msg_uuid > ?)"
    };

    let limit_clause = if recent || limit > 0 { "LIMIT ?" } else { "" };
    let inner_order = if recent { "DESC" } else { "ASC" };

    format!(
        r#"
        SELECT *
        FROM (
            SELECT
                m.msg_uuid,
                m.msg_server_id,
                m.conversation_id,
                COALESCE(
                    NULLIF(cc.name,''),
                    (
                        SELECT NULLIF(e.value,'')
                        FROM conversation_core_ext AS e
                        WHERE e.conversation_id = m.conversation_id
                          AND e.key = 'a:s_verify_group_name'
                        LIMIT 1
                    ),
                    m.conversation_id
                ) AS conversation_name,
                m.conversation_type,
                m.created_time,
                strftime('%Y-%m-%d %H:%M:%S', m.created_time / 1000, 'unixepoch', 'localtime') AS time_text,
                m.sender,
                COALESCE(NULLIF(p.alias,''), CAST(m.sender AS TEXT)) AS sender_name,
                m.type,
                m.content
            FROM msg AS m
            LEFT JOIN conversation_core AS cc ON cc.conversation_id = m.conversation_id
            LEFT JOIN participant AS p
                ON p.conversation_id = m.conversation_id AND p.user_id = m.sender
            {where_clause}
            ORDER BY m.created_time {inner_order}, m.msg_uuid {inner_order}
            {limit_clause}
        )
        ORDER BY created_time ASC, msg_uuid ASC
        "#
    )
}

fn hydrate_names(
    core_conn: &Connection,
    biz_conn: Option<&Connection>,
    self_uid: Option<i64>,
    messages: &mut [Message],
) {
    for message in messages {
        if let Some(sender) = message.sender {
            if let Some(name) = lookup_user_name(biz_conn, sender) {
                message.sender_name = name;
            }
        }
        if message.sender_name == message.sender.map(|v| v.to_string()).unwrap_or_default() {
            if let Some(name) = extract_sender_name_from_content(&message.content) {
                message.sender_name = name;
            }
        }

        if message.conversation_type == Some(1) || message.conversation_name == message.conversation_id {
            if let Some(peer_uid) = peer_uid_for_conversation(core_conn, &message.conversation_id, self_uid) {
                if let Some(peer_name) = lookup_user_name(biz_conn, peer_uid) {
                    message.conversation_name = format!("私信:{peer_name}");
                } else {
                    message.conversation_name = format!("私信:{peer_uid}");
                }
            }
        }
    }
}

fn hydrate_search_content(fts_conn: Option<&Connection>, messages: &mut [Message]) {
    let Some(conn) = fts_conn else {
        return;
    };
    for message in messages {
        if let Ok(search_content) = conn.query_row(
            "SELECT search_content FROM fts_search_msg_biz WHERE msg_uuid = ?",
            [&message.msg_uuid],
            |r| r.get::<_, String>(0),
        ) {
            if !search_content.trim().is_empty() {
                message.search_content = Some(search_content);
            }
        }
    }
}

fn lookup_user_name(biz_conn: Option<&Connection>, uid: i64) -> Option<String> {
    let conn = biz_conn?;
    conn.query_row(
        "SELECT COALESCE(NULLIF(REMARK_NAME,''), NULLIF(NICK_NAME,''), UID) FROM SIMPLE_USER WHERE UID = ?",
        [uid.to_string()],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .filter(|name| !name.trim().is_empty())
}

fn peer_uid_for_conversation(
    core_conn: &Connection,
    conversation_id: &str,
    self_uid: Option<i64>,
) -> Option<i64> {
    if let Some(uid) = self_uid {
        if let Ok(peer) = core_conn.query_row(
            "SELECT user_id FROM participant WHERE conversation_id = ? AND user_id != ? ORDER BY user_id LIMIT 1",
            params![conversation_id, uid],
            |r| r.get::<_, i64>(0),
        ) {
            return Some(peer);
        }
    }

    parse_peer_from_c2c_id(conversation_id, self_uid)
}

fn parse_peer_from_c2c_id(conversation_id: &str, self_uid: Option<i64>) -> Option<i64> {
    let mut parts = conversation_id.split(':');
    let _prefix0 = parts.next()?;
    let _prefix1 = parts.next()?;
    let a = parts.next()?.parse::<i64>().ok()?;
    let b = parts.next()?.parse::<i64>().ok()?;
    match self_uid {
        Some(uid) if uid == a => Some(b),
        Some(uid) if uid == b => Some(a),
        _ => Some(b),
    }
}

fn row_to_message(row: &Row<'_>) -> rusqlite::Result<Message> {
    let content: Option<String> = row.get(10)?;
    Ok(Message {
        msg_uuid: row.get(0)?,
        msg_server_id: row.get(1)?,
        conversation_id: row.get(2)?,
        conversation_name: row.get(3)?,
        conversation_type: row.get(4)?,
        created_time: row.get(5)?,
        time_text: row.get(6)?,
        sender: row.get(7)?,
        sender_name: row.get(8)?,
        msg_type: row.get(9)?,
        content: content.unwrap_or_default(),
        search_content: None,
    })
}

fn print_message(message: &Message, output: WatchOutput) -> Result<()> {
    match output {
        WatchOutput::Chat => {
            println!(
                "[{}] {} ({}) {} [{}]: {}",
                message.time_text,
                message.conversation_name,
                message.conversation_id,
                message.sender_name,
                message_type_name(message.msg_type),
                message_display_text(message)
            );
        }
        WatchOutput::Jsonl => {
            println!(
                "{{\"time\":\"{}\",\"created_time\":{},\"conversation_id\":\"{}\",\"conversation_name\":\"{}\",\"conversation_type\":{},\"sender\":{},\"sender_name\":\"{}\",\"type_name\":\"{}\",\"msg_type\":{},\"msg_uuid\":\"{}\",\"msg_server_id\":{},\"text\":\"{}\"}}",
                json_escape(&message.time_text),
                message.created_time,
                json_escape(&message.conversation_id),
                json_escape(&message.conversation_name),
                message.conversation_type.map(|v| v.to_string()).unwrap_or_else(|| "null".to_string()),
                message.sender.map(|v| v.to_string()).unwrap_or_else(|| "null".to_string()),
                json_escape(&message.sender_name),
                json_escape(message_type_name(message.msg_type)),
                message.msg_type.map(|v| v.to_string()).unwrap_or_else(|| "null".to_string()),
                json_escape(&message.msg_uuid),
                message.msg_server_id.map(|v| v.to_string()).unwrap_or_else(|| "null".to_string()),
                json_escape(&message_display_text(message)),
            );
        }
    }
    Ok(())
}

fn post_messages_if_needed(args: &WatchArgs, account_uid: &str, messages: &[Message]) {
    if messages.is_empty() {
        return;
    }
    let Some(url) = args.post_url.as_deref() else {
        return;
    };
    match post_messages(url, account_uid, messages, args.post_timeout_ms) {
        Ok(()) => eprintln!("[+] posted {} message(s)", messages.len()),
        Err(err) => eprintln!("[!] post failed: {err}"),
    }
}

fn post_messages(url: &str, account_uid: &str, messages: &[Message], timeout_ms: u64) -> Result<()> {
    let endpoint = parse_http_url(url)?;
    let body = build_post_body(account_uid, messages);
    let mut stream = TcpStream::connect((endpoint.host.as_str(), endpoint.port))?;
    let timeout = Duration::from_millis(timeout_ms);
    stream.set_read_timeout(Some(timeout)).ok();
    stream.set_write_timeout(Some(timeout)).ok();
    let request = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json; charset=utf-8\r\nAccept: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
        endpoint.path,
        endpoint.host_header,
        body.as_bytes().len(),
        body
    );
    stream.write_all(request.as_bytes())?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok();
    if response.starts_with("HTTP/1.1 2") || response.starts_with("HTTP/1.0 2") {
        Ok(())
    } else {
        let status = response.lines().next().unwrap_or("no response");
        anyhow::bail!("server returned {status}");
    }
}

fn parse_http_url(url: &str) -> Result<HttpEndpoint> {
    let Some(rest) = url.strip_prefix("http://") else {
        anyhow::bail!("only http:// URLs are supported by Android ELF poster");
    };
    let (host_port, path) = match rest.split_once('/') {
        Some((host_port, path)) => (host_port, format!("/{path}")),
        None => (rest, "/".to_string()),
    };
    if host_port.is_empty() {
        anyhow::bail!("missing host in post URL");
    }
    let (host, port) = match host_port.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() => (host.to_string(), port.parse::<u16>()?),
        _ => (host_port.to_string(), 80),
    };
    Ok(HttpEndpoint {
        host: host.clone(),
        port,
        path,
        host_header: if port == 80 { host } else { format!("{host}:{port}") },
    })
}

fn build_post_body(account_uid: &str, messages: &[Message]) -> String {
    let mut out = String::from("{\"messages\":[");
    for (idx, message) in messages.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        let direction = match message.sender.map(|v| v.to_string()) {
            Some(sender) if sender == account_uid => "out",
            _ if message.msg_type == Some(1) => "system",
            _ => "in",
        };
        out.push_str(&format!(
            "{{\"account_uid\":\"{}\",\"conversation_id\":\"{}\",\"conversation_type\":{},\"conversation_name\":\"{}\",\"msg_uuid\":\"{}\",\"msg_server_id\":{},\"created_time\":{},\"sender\":{},\"sender_name\":\"{}\",\"direction\":\"{}\",\"msg_type\":{},\"type_name\":\"{}\",\"text\":\"{}\"}}",
            json_escape(account_uid),
            json_escape(&message.conversation_id),
            message.conversation_type.map(|v| v.to_string()).unwrap_or_else(|| "null".to_string()),
            json_escape(&message.conversation_name),
            json_escape(&message.msg_uuid),
            message.msg_server_id.map(|v| format!("\"{v}\"")).unwrap_or_else(|| "null".to_string()),
            message.created_time,
            message.sender.map(|v| format!("\"{v}\"")).unwrap_or_else(|| "null".to_string()),
            json_escape(&message.sender_name),
            direction,
            message.msg_type.map(|v| v.to_string()).unwrap_or_else(|| "null".to_string()),
            json_escape(message_type_name(message.msg_type)),
            json_escape(&message_display_text(message)),
        ));
    }
    out.push_str("]}");
    out
}

fn message_display_text(message: &Message) -> String {
    let extracted = extract_message_text(&message.content, message.msg_type);
    if extracted != "[无可读文本]" {
        return extracted;
    }
    message
        .search_content
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("[无可读文本]")
        .to_string()
}

fn message_type_name(message_type: Option<i64>) -> &'static str {
    match message_type {
        Some(1) => "系统/提示",
        Some(5) => "表情/互动",
        Some(7) => "文本",
        Some(8) => "视频/作品分享",
        Some(17) => "语音",
        Some(21) => "直播/卡片",
        Some(25) => "主页/名片",
        Some(26) => "活动邀请",
        Some(27) => "图片",
        Some(30) => "业务卡片",
        Some(74) => "红包/奖励",
        Some(77) => "分享卡片",
        Some(110) => "动态卡片",
        Some(114) => "图文/内容分享",
        Some(122) => "现金红包",
        Some(136) => "聊天记录",
        Some(150) => "文件",
        Some(502) => "位置",
        Some(1001) => "群通知",
        Some(1004) => "群公告",
        _ => "未知类型",
    }
}

fn extract_message_text(content: &str, _message_type: Option<i64>) -> String {
    if let Ok(value) = serde_json::from_str::<Value>(content) {
        if let Some(text) = extract_text_from_value(&value) {
            return text;
        }
    }
    "[无可读文本]".to_string()
}

fn extract_sender_name_from_content(content: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(content).ok()?;
    for key in ["nickname", "nick_name", "display_name", "content_name"] {
        if let Some(text) = value.get(key).and_then(Value::as_str).map(str::trim) {
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }
    if let Some(raw) = value
        .get("im_dynamic_patch")
        .and_then(|v| v.get("raw_data"))
        .and_then(Value::as_str)
    {
        let raw_value = serde_json::from_str::<Value>(raw).ok()?;
        return first_string_for_keys(&raw_value, &["nickname", "nick_name", "title"]);
    }
    None
}

fn extract_text_from_value(value: &Value) -> Option<String> {
    for key in [
        "text",
        "notice_content",
        "upgraded_notice_content",
        "content_title",
        "description",
        "title",
        "desc",
        "hint_content",
        "push_detail",
        "cover_sub_title",
        "sup_rp_desc",
        "content_name",
        "display_name",
        "name",
        "poi_name",
        "poi_address",
    ] {
        if let Some(text) = value.get(key).and_then(Value::as_str).map(str::trim) {
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }

    if let Some(resources) = value.get("locale_resources").and_then(Value::as_array) {
        for item in resources {
            if item.get("lang").and_then(Value::as_str) == Some("zh-Hans") {
                if let Some(text) = item.get("text").and_then(Value::as_str).map(str::trim) {
                    if !text.is_empty() {
                        return Some(text.to_string());
                    }
                }
            }
        }
        for item in resources {
            if let Some(text) = item.get("text").and_then(Value::as_str).map(str::trim) {
                if !text.is_empty() {
                    return Some(text.to_string());
                }
            }
        }
    }

    if let Some(content_ext) = value.get("content_ext").and_then(Value::as_object) {
        for key in ["active_notice", "passive_notice"] {
            if let Some(text) = content_ext.get(key).and_then(Value::as_str).map(str::trim) {
                if !text.is_empty() {
                    return Some(text.to_string());
                }
            }
        }
    }

    if let Some(raw) = value
        .get("im_dynamic_patch")
        .and_then(|v| v.get("raw_data"))
        .and_then(Value::as_str)
    {
        if let Ok(raw_value) = serde_json::from_str::<Value>(raw) {
            return first_content_string(&raw_value);
        }
    }

    None
}

fn first_content_string(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => {
            if let Some(text) = map.get("content").and_then(Value::as_str).map(str::trim) {
                if !text.is_empty() && !text.starts_with("http") {
                    return Some(text.to_string());
                }
            }
            for nested in map.values() {
                if let Some(text) = first_content_string(nested) {
                    return Some(text);
                }
            }
            None
        }
        Value::Array(items) => {
            for item in items {
                if let Some(text) = first_content_string(item) {
                    return Some(text);
                }
            }
            None
        }
        _ => None,
    }
}

fn first_string_for_keys(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(text) = map.get(*key).and_then(Value::as_str).map(str::trim) {
                    if !text.is_empty() && !text.starts_with("http") {
                        return Some(text.to_string());
                    }
                }
            }
            for nested in map.values() {
                if let Some(text) = first_string_for_keys(nested, keys) {
                    return Some(text);
                }
            }
            None
        }
        Value::Array(items) => {
            for item in items {
                if let Some(text) = first_string_for_keys(item, keys) {
                    return Some(text);
                }
            }
            None
        }
        _ => None,
    }
}

fn json_escape(input: &str) -> String {
    let mut out = String::new();
    for ch in input.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

trait OptionalRow<T> {
    fn optional(self) -> rusqlite::Result<Option<T>>;
}

impl<T> OptionalRow<T> for rusqlite::Result<T> {
    fn optional(self) -> rusqlite::Result<Option<T>> {
        match self {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}
