//! 测试用的内存 IMAP 服务器：支持 LOGIN、CAPABILITY、SELECT、APPEND（含字面量）、
//! UID SEARCH（ALL、UNDELETED、UNSEEN、FLAGGED、HEADER、FROM、TO、SUBJECT、TEXT、CHARSET、OR，搜索词可用字面量）、UID STORE、UID EXPUNGE、UID FETCH（含 N:M、N:* 范围）、
//! UID FETCH (UID RFC822.SIZE [INTERNALDATE])、UID MOVE / UID COPY、STATUS 和 LOGOUT，
//! 并记录收到的每条命令，便于断言协议行为。

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, DuplexStream};

#[derive(Clone, Debug)]
pub struct StoredMessage {
    pub uid: u32,
    pub raw: Vec<u8>,
    pub flags: Vec<String>,
}

#[derive(Default)]
pub struct Mailbox {
    pub name: String,
    pub validity: u32,
    pub next_uid: u32,
    pub messages: Vec<StoredMessage>,
}

#[derive(Default)]
pub struct ServerState {
    pub mailboxes: Vec<Mailbox>,
    pub capabilities: String,
    pub commands: Vec<String>,
    /// 以这些前缀开头的命令回 NO，用于模拟服务器故障
    pub fail: Vec<String>,
    /// 收到以这些前缀开头的命令时直接断开连接，模拟网络故障
    pub disconnect: Vec<String>,
    /// 模拟慢服务器：一次 UID FETCH 超过这么多封就一直不回（客户端只能等到超时）
    pub stall_fetch_over: Option<usize>,
    /// 模拟拿不到正文的邮件：取这些 UID 的正文时一直不回
    pub stall_bodies: Vec<u32>,
}

pub type Shared = Arc<Mutex<ServerState>>;

pub fn state(capabilities: &str, mailboxes: &[(&str, u32)]) -> Shared {
    Arc::new(Mutex::new(ServerState {
        mailboxes: mailboxes
            .iter()
            .map(|(name, validity)| Mailbox {
                name: name.to_string(),
                validity: *validity,
                next_uid: 1,
                messages: Vec::new(),
            })
            .collect(),
        capabilities: capabilities.to_string(),
        commands: Vec::new(),
        fail: Vec::new(),
        disconnect: Vec::new(),
        stall_fetch_over: None,
        stall_bodies: Vec::new(),
    }))
}

impl ServerState {
    pub fn mailbox(&mut self, name: &str) -> &mut Mailbox {
        self.mailboxes
            .iter_mut()
            .find(|m| m.name == name)
            .expect("测试邮箱存在")
    }

    pub fn add(&mut self, mailbox: &str, raw: &str, flags: &[&str]) -> u32 {
        let mb = self.mailbox(mailbox);
        let uid = mb.next_uid;
        mb.next_uid += 1;
        mb.messages.push(StoredMessage {
            uid,
            raw: raw.as_bytes().to_vec(),
            flags: flags.iter().map(|f| f.to_string()).collect(),
        });
        uid
    }
}

fn header_value(raw: &[u8], name: &str) -> Option<String> {
    let text = String::from_utf8_lossy(raw);
    let head = text.split("\r\n\r\n").next().unwrap_or("");
    head.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim()
            .eq_ignore_ascii_case(name)
            .then(|| value.trim().to_string())
    })
}

/// 解析 UID 集合，`*` 按 `max`（邮箱里最大的 UID）处理。
fn uid_set_in(set: &str, max: u32) -> Vec<u32> {
    let bound = |v: &str| if v == "*" { Some(max) } else { v.parse().ok() };
    set.split(',')
        .flat_map(|part| match part.split_once(':') {
            Some((a, b)) => match (bound(a), bound(b)) {
                (Some(a), Some(b)) => (a.min(b)..=a.max(b)).collect(),
                _ => Vec::new(),
            },
            None => bound(part).into_iter().collect(),
        })
        .collect()
}

fn uid_set(set: &str) -> Vec<u32> {
    uid_set_in(set, 0)
}

/// 把 SEARCH 条件拆成词；引号里的内容是一个词。
fn search_tokens(query: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut chars = query.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c == ' ' {
            chars.next();
        } else if c == '"' {
            chars.next();
            let mut word = String::new();
            loop {
                match chars.next()? {
                    '\\' => word.push(chars.next()?),
                    '"' => break,
                    c => word.push(c),
                }
            }
            out.push(format!("\"{word}"));
        } else {
            let mut word = String::new();
            while let Some(&c) = chars.peek() {
                if c == ' ' {
                    break;
                }
                word.push(c);
                chars.next();
            }
            out.push(word);
        }
    }
    Some(out)
}

/// 只支持测试用到的条件：ALL、UNDELETED、UNSEEN、FLAGGED、HEADER 名字 值、FROM/TO/SUBJECT/TEXT 值、CHARSET、OR a b。
/// 不认识的条件返回 None（服务器回 BAD），免得测试误以为服务器支持。
fn search_matches(tokens: &[String], pos: &mut usize, m: &StoredMessage) -> Option<bool> {
    let has = |flag: &str| m.flags.iter().any(|f| f == flag);
    let word = tokens.get(*pos)?.to_ascii_uppercase();
    *pos += 1;
    Some(match word.as_str() {
        "ALL" => true,
        "UNDELETED" => !has("\\Deleted"),
        "UNSEEN" => !has("\\Seen"),
        "FLAGGED" => has("\\Flagged"),
        "OR" => {
            let a = search_matches(tokens, pos, m)?;
            let b = search_matches(tokens, pos, m)?;
            a || b
        }
        "CHARSET" => {
            // 测试数据都是 UTF-8，只认这一种
            if !tokens.get(*pos)?.eq_ignore_ascii_case("UTF-8") {
                return None;
            }
            *pos += 1;
            true
        }
        "FROM" | "TO" | "SUBJECT" => {
            let value = tokens.get(*pos)?.trim_start_matches('"').to_lowercase();
            *pos += 1;
            header_value(&m.raw, &word).is_some_and(|v| v.to_lowercase().contains(&value))
        }
        "TEXT" => {
            let value = tokens.get(*pos)?.trim_start_matches('"').to_lowercase();
            *pos += 1;
            String::from_utf8_lossy(&m.raw)
                .to_lowercase()
                .contains(&value)
        }
        "HEADER" => {
            let name = tokens.get(*pos)?.clone();
            let value = tokens.get(*pos + 1)?.trim_start_matches('"').to_lowercase();
            *pos += 2;
            header_value(&m.raw, &name).is_some_and(|v| v.to_lowercase().contains(&value))
        }
        _ => return None,
    })
}

/// 启动服务器，返回已登录的会话。
pub async fn session(state: Shared) -> async_imap::Session<DuplexStream> {
    let (client, server) = tokio::io::duplex(1 << 20);
    tokio::spawn(serve(server, state));
    let mut client = async_imap::Client::new(client);
    client.read_response().await.unwrap();
    client
        .login("test", "pw")
        .await
        .map_err(|(e, _)| e)
        .unwrap()
}

async fn serve(stream: DuplexStream, state: Shared) {
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);
    writer.write_all(b"* OK test server\r\n").await.unwrap();
    let mut selected: Option<String> = None;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
            return;
        }
        let mut trimmed = line.trim_end().to_string();
        // 搜索词可以用字面量（{N}）发送：先回 "+"，读 N 个字节再接着读这一行剩下的部分。
        // 像严格的服务器（如网易 Coremail）一样，引号字符串里出现非 ASCII 字符直接回 BAD，
        // 因为 RFC 3501 规定引号字符串只能是 7 位字符。字面量内容拼回去时转成引号字符串。
        if trimmed.to_ascii_uppercase().contains(" UID SEARCH ") {
            let mut assembled = String::new();
            let mut eight_bit_quoted = false;
            loop {
                let size = trimmed
                    .strip_suffix('}')
                    .and_then(|t| t.rsplit_once('{'))
                    .and_then(|(head, n)| n.parse::<usize>().ok().map(|n| (head.to_string(), n)));
                let Some((head, size)) = size else {
                    eight_bit_quoted |= !trimmed.is_ascii();
                    assembled.push_str(&trimmed);
                    break;
                };
                eight_bit_quoted |= !head.is_ascii();
                assembled.push_str(&head);
                writer.write_all(b"+ go ahead\r\n").await.unwrap();
                let mut literal = vec![0u8; size];
                reader.read_exact(&mut literal).await.unwrap();
                let literal = String::from_utf8_lossy(&literal);
                assembled.push_str(&format!(
                    "\"{}\"",
                    literal.replace('\\', "\\\\").replace('"', "\\\"")
                ));
                line.clear();
                reader.read_line(&mut line).await.unwrap();
                trimmed = line.trim_end().to_string();
            }
            if eight_bit_quoted {
                let tag = assembled.split(' ').next().unwrap_or("*").to_string();
                state.lock().unwrap().commands.push(assembled);
                writer
                    .write_all(
                        format!("{tag} BAD 8-bit characters in quoted string\r\n").as_bytes(),
                    )
                    .await
                    .unwrap();
                continue;
            }
            trimmed = assembled;
        }
        let Some((tag, command)) = trimmed.split_once(' ') else {
            continue;
        };
        let (tag, command) = (tag.to_string(), command.to_string());
        state.lock().unwrap().commands.push(command.clone());
        let upper = command.to_ascii_uppercase();
        let failing = state
            .lock()
            .unwrap()
            .fail
            .iter()
            .any(|prefix| upper.starts_with(&prefix.to_ascii_uppercase()));
        if state
            .lock()
            .unwrap()
            .disconnect
            .iter()
            .any(|prefix| upper.starts_with(&prefix.to_ascii_uppercase()))
        {
            return;
        }
        let reply = if failing && !upper.starts_with("APPEND") {
            format!("{tag} NO simulated failure\r\n")
        } else if upper.starts_with("LOGIN") {
            format!("{tag} OK logged in\r\n")
        } else if upper == "CAPABILITY" {
            let caps = state.lock().unwrap().capabilities.clone();
            format!("* CAPABILITY IMAP4rev1 {caps}\r\n{tag} OK done\r\n")
        } else if upper.starts_with("SELECT") || upper.starts_with("EXAMINE") {
            let name = command
                .split_once(' ')
                .map(|(_, n)| n.trim_matches('"').to_string())
                .unwrap_or_default();
            let mut st = state.lock().unwrap();
            match st.mailboxes.iter().find(|m| m.name == name) {
                Some(mb) => {
                    let reply = format!(
                        "* {} EXISTS\r\n* OK [UIDVALIDITY {}] ok\r\n* OK [UIDNEXT {}] ok\r\n{tag} OK [READ-WRITE] selected\r\n",
                        mb.messages.len(),
                        mb.validity,
                        mb.next_uid
                    );
                    selected = Some(name);
                    reply
                }
                None => {
                    st.commands.push(format!("(no mailbox {name})"));
                    format!("{tag} NO no such mailbox\r\n")
                }
            }
        } else if upper.starts_with("APPEND") {
            // APPEND "name" (flags) {N}
            let name = command
                .split('"')
                .nth(1)
                .map(str::to_string)
                .unwrap_or_default();
            let flags: Vec<String> = command
                .split_once('(')
                .and_then(|(_, rest)| rest.split_once(')'))
                .map(|(f, _)| f.split_whitespace().map(str::to_string).collect())
                .unwrap_or_default();
            let size: usize = command
                .rsplit_once('{')
                .and_then(|(_, n)| n.trim_end_matches('}').trim_end_matches('+').parse().ok())
                .unwrap_or(0);
            writer.write_all(b"+ go ahead\r\n").await.unwrap();
            let mut raw = vec![0u8; size];
            reader.read_exact(&mut raw).await.unwrap();
            let mut end = String::new();
            reader.read_line(&mut end).await.unwrap();
            let mut st = state.lock().unwrap();
            let caps = st.capabilities.clone();
            let mb = st.mailbox(&name);
            let uid = mb.next_uid;
            mb.next_uid += 1;
            mb.messages.push(StoredMessage { uid, raw, flags });
            if caps.contains("UIDPLUS") {
                format!("{tag} OK [APPENDUID {} {uid}] appended\r\n", mb.validity)
            } else {
                format!("{tag} OK appended\r\n")
            }
        } else if upper.starts_with("UID SEARCH") {
            let mut st = state.lock().unwrap();
            let name = selected.clone().unwrap_or_default();
            let mb = st.mailbox(&name);
            let query = command["UID SEARCH".len()..].trim().to_string();
            match search_tokens(&query).and_then(|tokens| {
                let mut uids = Vec::new();
                for m in &mb.messages {
                    let mut pos = 0;
                    let mut all = true;
                    while pos < tokens.len() {
                        all &= search_matches(&tokens, &mut pos, m)?;
                    }
                    if all {
                        uids.push(m.uid);
                    }
                }
                Some(uids)
            }) {
                Some(uids) => {
                    let list = uids
                        .iter()
                        .map(u32::to_string)
                        .collect::<Vec<_>>()
                        .join(" ");
                    format!("* SEARCH {list}\r\n{tag} OK searched\r\n")
                }
                None => format!("{tag} BAD unsupported search criteria\r\n"),
            }
        } else if upper.starts_with("UID STORE") {
            let parts: Vec<&str> = command.split_whitespace().collect();
            let set = uid_set(parts.get(2).copied().unwrap_or(""));
            let flags: Vec<String> = command
                .split_once('(')
                .and_then(|(_, rest)| rest.split_once(')'))
                .map(|(f, _)| f.split_whitespace().map(str::to_string).collect())
                .unwrap_or_default();
            let mut st = state.lock().unwrap();
            let name = selected.clone().unwrap_or_default();
            for m in st
                .mailbox(&name)
                .messages
                .iter_mut()
                .filter(|m| set.contains(&m.uid))
            {
                for f in &flags {
                    if parts.get(3).is_some_and(|op| op.starts_with('-')) {
                        m.flags.retain(|old| !old.eq_ignore_ascii_case(f));
                    } else if !m.flags.contains(f) {
                        m.flags.push(f.clone());
                    }
                }
            }
            format!("{tag} OK stored\r\n")
        } else if upper.starts_with("UID MOVE") || upper.starts_with("UID COPY") {
            // UID MOVE|COPY set "target"
            let set = uid_set(command.split_whitespace().nth(2).unwrap_or(""));
            let target = command.split('"').nth(1).unwrap_or_default().to_string();
            let moving = upper.starts_with("UID MOVE");
            let mut st = state.lock().unwrap();
            let caps = st.capabilities.clone();
            if moving && !caps.contains("MOVE") {
                format!("{tag} BAD MOVE not supported\r\n")
            } else if !st.mailboxes.iter().any(|m| m.name == target) {
                format!("{tag} NO [TRYCREATE] no such mailbox\r\n")
            } else {
                let name = selected.clone().unwrap_or_default();
                let picked: Vec<StoredMessage> = st
                    .mailbox(&name)
                    .messages
                    .iter()
                    .filter(|m| set.contains(&m.uid))
                    .cloned()
                    .collect();
                if moving {
                    st.mailbox(&name).messages.retain(|m| !set.contains(&m.uid));
                }
                let dest = st.mailbox(&target);
                let copied_from = picked.iter().map(|m| m.uid.to_string()).collect::<Vec<_>>();
                let mut copied_to = Vec::new();
                for mut m in picked {
                    m.uid = dest.next_uid;
                    copied_to.push(m.uid.to_string());
                    dest.next_uid += 1;
                    dest.messages.push(m);
                }
                if caps.contains("UIDPLUS") && !copied_from.is_empty() {
                    format!(
                        "{tag} OK [COPYUID {} {} {}] done\r\n",
                        dest.validity,
                        copied_from.join(","),
                        copied_to.join(",")
                    )
                } else {
                    format!("{tag} OK done\r\n")
                }
            }
        } else if upper.starts_with("UID EXPUNGE") {
            let set = uid_set(command.split_whitespace().nth(2).unwrap_or(""));
            let mut st = state.lock().unwrap();
            let name = selected.clone().unwrap_or_default();
            st.mailbox(&name)
                .messages
                .retain(|m| !(set.contains(&m.uid) && m.flags.iter().any(|f| f == "\\Deleted")));
            format!("{tag} OK expunged\r\n")
        } else if upper.starts_with("STATUS") {
            let name = command.split('"').nth(1).unwrap_or_default().to_string();
            let st = state.lock().unwrap();
            match st.mailboxes.iter().find(|m| m.name == name) {
                Some(mb) => format!(
                    "* STATUS \"{name}\" (UIDNEXT {} UIDVALIDITY {})\r\n{tag} OK status\r\n",
                    mb.next_uid, mb.validity
                ),
                None => format!("{tag} NO no such mailbox\r\n"),
            }
        } else if upper.starts_with("UID FETCH")
            && upper.contains("RFC822.SIZE")
            && !upper.contains("BODY")
        {
            // 只问大小：UID FETCH set (UID RFC822.SIZE)
            let mut st = state.lock().unwrap();
            let name = selected.clone().unwrap_or_default();
            let mb = st.mailbox(&name);
            let max = mb.messages.iter().map(|m| m.uid).max().unwrap_or(0);
            let set = uid_set_in(command.split_whitespace().nth(2).unwrap_or(""), max);
            let mut out = String::new();
            let with_date = upper.contains("INTERNALDATE");
            for (i, m) in mb.messages.iter().enumerate() {
                if set.contains(&m.uid) {
                    let date = if with_date {
                        format!(
                            " INTERNALDATE \"{}\"",
                            header_value(&m.raw, "X-Internal-Date")
                                .unwrap_or_else(|| "01-Jan-2026 00:00:00 +0000".into())
                        )
                    } else {
                        String::new()
                    };
                    out.push_str(&format!(
                        "* {} FETCH (UID {} RFC822.SIZE {}{date})\r\n",
                        i + 1,
                        m.uid,
                        m.raw.len()
                    ));
                }
            }
            format!("{out}{tag} OK fetched\r\n")
        } else if upper.starts_with("UID FETCH")
            && upper.contains("FLAGS")
            && !upper.contains("BODY")
        {
            // 只问标记：UID FETCH set (UID FLAGS)
            let mut st = state.lock().unwrap();
            let name = selected.clone().unwrap_or_default();
            let mb = st.mailbox(&name);
            let max = mb.messages.iter().map(|m| m.uid).max().unwrap_or(0);
            let set = uid_set_in(command.split_whitespace().nth(2).unwrap_or(""), max);
            let mut out = String::new();
            for (i, m) in mb.messages.iter().enumerate() {
                if set.contains(&m.uid) {
                    out.push_str(&format!(
                        "* {} FETCH (UID {} FLAGS ({}))\r\n",
                        i + 1,
                        m.uid,
                        m.flags.join(" ")
                    ));
                }
            }
            format!("{out}{tag} OK fetched\r\n")
        } else if upper.starts_with("UID FETCH") {
            let header_only = upper.contains("BODY.PEEK[HEADER]");
            let out = {
                let mut st = state.lock().unwrap();
                let name = selected.clone().unwrap_or_default();
                // RFC 3501：N:* 在没有更大 UID 时也至少返回最后一封
                let max = st
                    .mailbox(&name)
                    .messages
                    .iter()
                    .map(|m| m.uid)
                    .max()
                    .unwrap_or(0);
                let set = uid_set_in(command.split_whitespace().nth(2).unwrap_or(""), max);
                let wanted = st
                    .mailbox(&name)
                    .messages
                    .iter()
                    .filter(|m| set.contains(&m.uid))
                    .count();
                let stall = st.stall_fetch_over.is_some_and(|n| wanted > n)
                    || (!header_only && st.stall_bodies.iter().any(|uid| set.contains(uid)));
                if stall {
                    None
                } else {
                    let mut out = Vec::new();
                    for (i, m) in st.mailbox(&name).messages.iter().enumerate() {
                        if !set.contains(&m.uid) {
                            continue;
                        }
                        let body: Vec<u8> = if header_only {
                            let text = String::from_utf8_lossy(&m.raw);
                            let head = text.split("\r\n\r\n").next().unwrap_or("");
                            format!("{head}\r\n\r\n").into_bytes()
                        } else {
                            m.raw.clone()
                        };
                        out.extend_from_slice(
                            format!(
                                "* {} FETCH (UID {} FLAGS ({}) INTERNALDATE \"{}\" {} {{{}}}\r\n",
                                i + 1,
                                m.uid,
                                m.flags.join(" "),
                                // 测试可以用 X-Internal-Date 头指定服务器收件时间
                                header_value(&m.raw, "X-Internal-Date")
                                    .unwrap_or_else(|| "01-Jan-2026 00:00:00 +0000".into()),
                                if header_only {
                                    "BODY[HEADER]"
                                } else {
                                    "BODY[]"
                                },
                                body.len()
                            )
                            .as_bytes(),
                        );
                        out.extend_from_slice(&body);
                        out.extend_from_slice(b")\r\n");
                    }
                    out.extend_from_slice(format!("{tag} OK fetched\r\n").as_bytes());
                    Some(out)
                }
            };
            let Some(out) = out else {
                // 不回也不断开：客户端只能靠自己的超时发现
                std::future::pending::<()>().await;
                unreachable!()
            };
            writer.write_all(&out).await.unwrap();
            continue;
        } else if upper == "LOGOUT" {
            let _ = writer
                .write_all(format!("* BYE\r\n{tag} OK bye\r\n").as_bytes())
                .await;
            return;
        } else if upper == "NOOP" {
            format!("{tag} OK noop\r\n")
        } else {
            format!("{tag} BAD unsupported in test server\r\n")
        };
        if writer.write_all(reply.as_bytes()).await.is_err() {
            return;
        }
    }
}
