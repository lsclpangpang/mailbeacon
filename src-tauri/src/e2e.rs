//! 端到端测试：连一台真实的 IMAP/SMTP 服务器跑完整流程。默认跳过，需要时：
//!
//! ```sh
//! MAILPULSE_E2E=localhost:3993:3465 MAILPULSE_EXTRA_CA=ca.pem \
//!   cargo test -p mailpulse-app e2e -- --ignored --nocapture
//! ```
//! 服务器上要有 me@localhost / bob@localhost，进度与文件名测试另需空的
//! progress@localhost / names@localhost / inherit@localhost 隔离账号，密码都是 pw（GreenMail 即可）。

use std::time::{Duration, Instant};

use async_imap::extensions::idle::IdleResponse;
use tokio::time::timeout;

use crate::{
    config::Account,
    imap::{self, OP_TIMEOUT},
    smtp,
};

/// 真实 IMAP 原文以主题命名；同主题、相同原文不同 UID 仍分别保存，兼容旧版文件名。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_bulk_download_subject_filenames() {
    use crate::{auto_download::Checkpoint, bulk_download};
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use std::{fs, sync::Arc};

    let me = account("names");
    let mut session = imap::login(&me, "pw").await.unwrap();
    let duplicate = b"From: names@localhost\r\nSubject: =?UTF-8?B?5rWL6K+V?=\r\n\r\nSame bytes, distinct UID\r\n";
    let legacy_raw =
        "From: names@localhost\r\nSubject: 旧归档测试\r\n\r\nLegacy original\r\n".as_bytes();
    let empty_subject = b"From: names@localhost\r\n\r\nNo subject\r\n";
    for raw in [
        duplicate.as_slice(),
        duplicate.as_slice(),
        legacy_raw,
        empty_subject.as_slice(),
    ] {
        session.append("INBOX", None, None, raw).await.unwrap();
    }
    let mailbox = session.select("INBOX").await.unwrap();
    let mut uids: Vec<_> = session
        .uid_search("ALL")
        .await
        .unwrap()
        .into_iter()
        .collect();
    uids.sort_unstable();
    assert_eq!(uids.len(), 4);
    let unread = session.uid_search("UNSEEN").await.unwrap();
    let parent = tempfile::tempdir().unwrap();
    let root = bulk_download::account_directory(parent.path(), &me).unwrap();
    let folder = imap::Folder {
        path: "INBOX".into(),
        name: "INBOX".into(),
        role: "other".into(),
    };
    let directory = bulk_download::stable_folder_directory(&root, &folder).unwrap();
    let checkpoint = Checkpoint {
        uid_validity: mailbox.uid_validity.unwrap(),
        last_uid: *uids.last().unwrap(),
    };
    let old_path = directory.join(format!(
        "MailBeacon-{}-{}-{}.eml",
        URL_SAFE_NO_PAD.encode(me.id.as_bytes()),
        checkpoint.uid_validity,
        uids[2]
    ));
    fs::write(&old_path, legacy_raw).unwrap();

    let first = bulk_download::run_session(
        &mut session,
        &me,
        parent.path(),
        false,
        Arc::default(),
        |_| {},
    )
    .await;
    assert_eq!(first.state, "complete", "{}", first.detail);
    assert_eq!(
        (
            first.saved_count,
            first.existing_count,
            first.total_count,
            first.processed_count
        ),
        (3, 1, Some(4), 4)
    );
    assert!(!old_path.exists(), "旧版名称应在核对原文后迁移");
    let expected = [
        ("测试.eml".to_string(), duplicate.as_slice()),
        ("测试 (2).eml".to_string(), duplicate.as_slice()),
        ("旧归档测试.eml".to_string(), legacy_raw),
        (format!("邮件-{}.eml", uids[3]), empty_subject.as_slice()),
    ];
    for (name, raw) in &expected {
        assert_eq!(
            fs::read(directory.join(name)).unwrap(),
            *raw,
            "主题命名不得改变邮件原文字节"
        );
    }
    let count_emls = || {
        fs::read_dir(&directory)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "eml"))
            .count()
    };
    assert_eq!(count_emls(), 4, "相同原文不同UID也要分别保存");
    let second = bulk_download::run_session(
        &mut session,
        &me,
        parent.path(),
        false,
        Arc::default(),
        |_| {},
    )
    .await;
    assert_eq!(second.state, "complete", "{}", second.detail);
    assert_eq!((second.saved_count, second.existing_count), (0, 4));
    assert_eq!(count_emls(), 4, "重试不得重复生成主题文件");
    session.select("INBOX").await.unwrap();
    assert_eq!(session.uid_search("UNSEEN").await.unwrap(), unread);
    if let Ok(output) = std::env::var("MAILPULSE_E2E_ARTIFACTS") {
        let output = std::path::Path::new(&output).join("subject-names");
        fs::create_dir_all(&output).unwrap();
        for (name, _) in &expected {
            fs::copy(directory.join(name), output.join(name)).unwrap();
        }
    }
    let _ = session.logout().await;
    println!(
        "主题文件名通过：中文编码头、同主题与相同原文不同UID分别编号、空主题回退、旧文件迁移、重复不增文件、字节完整与未读不变"
    );
}

/// 邮件总数使用真实 UID 集合；扫描后新增或删除邮件不改变本次分母。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_bulk_download_progress_snapshot() {
    use crate::bulk_download;
    use futures::TryStreamExt;
    use std::sync::Arc;

    let me = account("progress");
    let mut session = imap::login(&me, "pw").await.unwrap();
    let empty_parent = tempfile::tempdir().unwrap();
    let empty = bulk_download::run_session(
        &mut session,
        &me,
        empty_parent.path(),
        false,
        Arc::default(),
        |_| {},
    )
    .await;
    assert_eq!(empty.state, "complete");
    assert_eq!(empty.total_count, Some(0));
    assert_eq!(empty.processed_count, 0);

    for name in ["Archive", "Empty"] {
        session.create(name).await.unwrap();
    }
    for index in 0..5 {
        let raw =
            format!("From: progress@localhost\r\nSubject: snapshot-{index}\r\n\r\nbody-{index}");
        session
            .append(
                if index < 3 { "INBOX" } else { "Archive" },
                None,
                None,
                raw.as_bytes(),
            )
            .await
            .unwrap();
    }
    session.select("INBOX").await.unwrap();
    let mut inbox: Vec<_> = session
        .uid_search("ALL")
        .await
        .unwrap()
        .into_iter()
        .collect();
    inbox.sort_unstable();
    let _: Vec<_> = session
        .uid_store(inbox[1].to_string(), "+FLAGS.SILENT (\\Deleted)")
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let _: Vec<_> = session
        .expunge()
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let mut changer = imap::login(&me, "pw").await.unwrap();
    changer.select("Archive").await.unwrap();
    let removed_uid = *changer
        .uid_search("ALL")
        .await
        .unwrap()
        .iter()
        .min()
        .unwrap();

    let parent = tempfile::tempdir().unwrap();
    let mut changed_after_scan = false;
    let mut states = Vec::new();
    let result = bulk_download::run_session(&mut session, &me, parent.path(), false, Arc::default(), |status| {
        if status.total_count.is_some() && !changed_after_scan {
            assert_eq!(status.total_count, Some(4), "UID中间有空洞，不能把UIDNEXT当邮件数");
            assert_eq!(status.processed_count, 0);
            tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(async {
                let _: Vec<_> = changer.uid_store(removed_uid.to_string(), "+FLAGS.SILENT (\\Deleted)").await.unwrap().try_collect().await.unwrap();
                let _: Vec<_> = changer.expunge().await.unwrap().try_collect().await.unwrap();
                changer.append("INBOX", None, None, b"From: progress@localhost\r\nSubject: after-snapshot\r\n\r\nnew arrival").await.unwrap();
            }));
            changed_after_scan = true;
        }
        states.push(status);
    }).await;
    assert!(changed_after_scan);
    assert!(
        states
            .iter()
            .any(|status| status.state == "scanning" && status.total_count.is_none())
    );
    assert_eq!(result.state, "complete", "{}", result.detail);
    assert_eq!(result.total_count, Some(4));
    assert_eq!(
        (
            result.processed_count,
            result.saved_count,
            result.vanished_count,
            result.skipped_count,
            result.failed_count
        ),
        (4, 3, 1, 0, 0)
    );
    assert_eq!(result.folders_scanned, result.folder_count);
    assert!(
        states
            .windows(2)
            .all(|pair| pair[1].processed_count >= pair[0].processed_count)
    );
    for state in &states {
        if let Some(total) = state.total_count {
            assert!(state.processed_count <= total);
        }
    }
    // 取消统计不报告假总数，也不写入邮件。
    let control = Arc::new(bulk_download::Control::default());
    let stop = control.clone();
    let cancel_parent = tempfile::tempdir().unwrap();
    let cancelled = bulk_download::run_session(
        &mut session,
        &me,
        cancel_parent.path(),
        false,
        control,
        move |status| {
            if status.state == "scanning" && status.current_folder.is_some() {
                stop.cancel();
            }
        },
    )
    .await;
    assert_eq!(cancelled.state, "cancelled");
    assert_eq!(cancelled.total_count, None);
    assert_eq!((cancelled.saved_count, cancelled.processed_count), (0, 0));
    let _ = session.logout().await;
    let _ = changer.logout().await;
    println!(
        "总量进度通过：空邮箱0封、UID空洞正确计数、固定总数4封、3封保存1封服务器移除、扫描后新邮件不计入、处理数单调且不超总数、统计期间可取消"
    );
}

/// 同一账号的历史下载可选择继承保存的规则，关闭自动下载仍可过滤。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_bulk_download_inherited_filters() {
    use crate::{auto_download::FilterMode, bulk_download};
    use std::{fs, path::Path, sync::Arc};

    fn contents(dir: &Path) -> Vec<Vec<u8>> {
        let mut messages = Vec::new();
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                messages.extend(contents(&path));
            } else if path.extension().is_some_and(|ext| ext == "eml") {
                messages.push(fs::read(path).unwrap());
            }
        }
        messages.sort();
        messages
    }

    let mut me = account("inherit");
    me.auto_download.enabled = false;
    me.auto_download.filter_mode = FilterMode::Whitelist;
    me.auto_download.keywords = vec!["WaNtEd".into()];
    let mut session = imap::login(&me, "pw").await.unwrap();
    let folder = imap::encode_mutf7("继承规则/历史邮件");
    session.create(&folder).await.unwrap();
    let messages: Vec<Vec<u8>> = [
        "From: bob@localhost\r\nSubject: wanted invoice\r\n\r\nordinary body",
        "From: wanted@localhost\r\nSubject: receipt\r\n\r\nordinary body",
        "From: bob@localhost\r\nSubject: report\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>WANTED 正文</p>",
        "From: bob@localhost\r\nSubject: news\r\n\r\nordinary body",
        "From: bob@localhost\r\nSubject: attachment\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: text/html\r\n\r\n<script>wanted</script><p>ordinary body</p>\r\n--x\r\nContent-Type: text/plain\r\nContent-Disposition: attachment; filename=note.txt\r\n\r\nwanted\r\n--x--\r\n",
    ].iter().map(|text| text.as_bytes().to_vec()).collect();
    for (index, raw) in messages.iter().enumerate() {
        session
            .append(
                if index % 2 == 0 { "INBOX" } else { &folder },
                None,
                None,
                raw,
            )
            .await
            .unwrap();
    }
    let mut unread = Vec::new();
    for name in ["INBOX", &folder] {
        session.select(name).await.unwrap();
        unread.push(session.uid_search("UNSEEN").await.unwrap());
    }
    for (mode, inherit, selected) in [
        (FilterMode::Whitelist, true, vec![0, 1, 2]),
        (FilterMode::Blacklist, true, vec![3, 4]),
        (FilterMode::Blacklist, false, vec![0, 1, 2, 3, 4]),
        (FilterMode::None, true, vec![0, 1, 2, 3, 4]),
    ] {
        me.auto_download.filter_mode = mode;
        let before = serde_json::to_value(&me.auto_download).unwrap();
        let parent = tempfile::tempdir().unwrap();
        let result = bulk_download::run_session(
            &mut session,
            &me,
            parent.path(),
            inherit,
            Arc::default(),
            |_| {},
        )
        .await;
        assert_eq!(result.state, "complete", "{}", result.detail);
        assert_eq!(result.inherit_filters, inherit);
        assert_eq!(
            result.filter_mode,
            if inherit { mode } else { FilterMode::None }
        );
        assert_eq!(result.skipped_count, (5 - selected.len()) as u64);
        assert_eq!(result.saved_count, selected.len() as u64);
        assert_eq!(result.failed_count, 0);
        assert_eq!(result.total_count, Some(5));
        assert_eq!(result.processed_count, 5, "命中规则跳过的邮件也算已处理");
        let mut expected: Vec<_> = selected
            .iter()
            .map(|index| messages[*index].clone())
            .collect();
        expected.sort();
        assert_eq!(
            contents(parent.path()),
            expected,
            "只保存匹配的原始邮件及附件字节"
        );
        assert_eq!(serde_json::to_value(&me.auto_download).unwrap(), before);
        // 在已有全部邮件的目录切换为白名单，不删除旧文件，只计符合规则的已有邮件。
        if !inherit {
            me.auto_download.filter_mode = FilterMode::Whitelist;
            let again = bulk_download::run_session(
                &mut session,
                &me,
                parent.path(),
                true,
                Arc::default(),
                |_| {},
            )
            .await;
            assert_eq!(again.state, "complete");
            assert_eq!(
                (again.saved_count, again.existing_count, again.skipped_count),
                (0, 3, 2)
            );
            assert_eq!(contents(parent.path()), expected);
        }
    }
    for (index, name) in ["INBOX", &folder].into_iter().enumerate() {
        session.select(name).await.unwrap();
        assert_eq!(session.uid_search("UNSEEN").await.unwrap(), unread[index]);
    }
    let broken = b"From: bob@localhost\r\nSubject: broken\r\nContent-Type: text/plain\r\nContent-Transfer-Encoding: base64\r\n\r\n%%%%";
    session.append("INBOX", None, None, broken).await.unwrap();
    me.auto_download.filter_mode = FilterMode::Whitelist;
    let parent = tempfile::tempdir().unwrap();
    let result = bulk_download::run_session(
        &mut session,
        &me,
        parent.path(),
        true,
        Arc::default(),
        |_| {},
    )
    .await;
    assert_eq!(
        result.state, "error",
        "解码失败不能伪装成规则跳过或全部完成"
    );
    assert_eq!(
        (
            result.saved_count,
            result.skipped_count,
            result.failed_count
        ),
        (3, 2, 1)
    );
    assert_eq!(contents(parent.path()).len(), 3);
    assert_eq!(result.total_count, Some(6));
    assert_eq!(
        result.processed_count, 6,
        "单封解码失败计入已处理，但整体仍报错"
    );
    let _ = session.logout().await;
    println!(
        "全量下载继承规则通过：白名单3存2跳过、黑名单2存3跳过、不继承与无规则各存5、自动下载关闭仍过滤、跨文件夹、原文附件完整、重跑不删文件、未读不变、解码失败明确报错"
    );
}

/// 全量导出跨文件夹、覆盖500封以上、不受自动过滤限制；取消、重跑和冲突不丢已有文件。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_bulk_download_all_folders_resume_and_cancel() {
    use crate::{auto_download, bulk_download};
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::Arc,
    };

    fn emls(dir: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    files.extend(emls(&path));
                } else if path.extension().is_some_and(|ext| ext == "eml") {
                    files.push(path);
                }
            }
        }
        files.sort();
        files
    }

    let mut me = account("bob");
    me.auto_download.filter_mode = auto_download::FilterMode::Whitelist;
    me.auto_download.keywords = vec!["definitely-no-message-matches-this-rule".into()];
    let automatic_before = serde_json::to_value(&me.auto_download).unwrap();
    let mut session = imap::login(&me, "pw").await.unwrap();
    session.select("INBOX").await.unwrap();
    let tag = chrono::Utc::now().timestamp_nanos_opt().unwrap();
    let mut folders = vec!["INBOX".to_string()];
    for name in ["Sent", "Drafts", "Trash", "Junk", "项目/资料", "财务/资料"] {
        let path = imap::encode_mutf7(name);
        if !imap::list_folders(&mut session)
            .await
            .unwrap()
            .iter()
            .any(|f| f.path == path)
        {
            session.create(&path).await.unwrap();
        }
        folders.push(path);
    }
    let empty_folder = imap::encode_mutf7(&format!("空文件夹-{tag}"));
    session.create(&empty_folder).await.unwrap();
    let mut expected = Vec::new();
    for (index, folder) in folders.iter().enumerate() {
        for number in 0..if index == 0 { 525 } else { 1 } {
            let raw = format!(
                "From: bob@localhost\r\nSubject: bulk-{tag}-{index}-{number}\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>完整邮件 <b>{index}/{number}</b></p>\r\n--x\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=data.bin\r\nContent-Transfer-Encoding: base64\r\n\r\nAP8=\r\n--x--\r\n"
            );
            session
                .append(folder, None, None, raw.as_bytes())
                .await
                .unwrap();
            expected.push(raw.into_bytes());
        }
    }
    let mut unread_before = Vec::new();
    for folder in &folders {
        session.select(folder).await.unwrap();
        unread_before.push(session.uid_search("UNSEEN").await.unwrap());
    }
    let parent = tempfile::tempdir().unwrap();
    let first = bulk_download::run_session(
        &mut session,
        &me,
        parent.path(),
        false,
        Arc::default(),
        |_| {},
    )
    .await;
    assert_eq!(first.state, "complete", "{}", first.detail);
    assert_eq!(first.failed_count, 0);
    let files = emls(parent.path());
    assert_eq!(first.saved_count as usize, files.len());
    assert_eq!(first.total_count, Some(files.len() as u64));
    assert_eq!(first.processed_count, files.len() as u64);
    assert!(first.folder_count >= folders.len() as u64);
    let archived: Vec<_> = files.iter().map(|path| fs::read(path).unwrap()).collect();
    for raw in &expected {
        assert!(
            archived.contains(raw),
            "全部文件夹邮件与附件原始字节完整保存"
        );
    }
    assert!(
        files.len() >= 531,
        "须处理500封以上的邮件，不能只导出列表首100封"
    );
    for (index, folder) in folders.iter().enumerate() {
        session.select(folder).await.unwrap();
        assert_eq!(
            session.uid_search("UNSEEN").await.unwrap(),
            unread_before[index]
        );
    }
    assert_eq!(
        serde_json::to_value(&me.auto_download).unwrap(),
        automatic_before
    );

    let second = bulk_download::run_session(
        &mut session,
        &me,
        parent.path(),
        false,
        Arc::default(),
        |_| {},
    )
    .await;
    assert_eq!(second.state, "complete", "{}", second.detail);
    assert_eq!(second.saved_count, 0);
    assert_eq!(second.existing_count as usize, files.len());
    assert_eq!(second.total_count, first.total_count);
    assert_eq!(second.processed_count, first.processed_count);
    assert_eq!(emls(parent.path()), files, "重复运行不得生成重复文件");

    let resume_parent = tempfile::tempdir().unwrap();
    let control = Arc::new(bulk_download::Control::default());
    let stop = control.clone();
    let cancelled = bulk_download::run_session(
        &mut session,
        &me,
        resume_parent.path(),
        false,
        control,
        move |status| {
            if status.saved_count >= 3 {
                stop.cancel();
            }
        },
    )
    .await;
    assert_eq!(cancelled.state, "cancelled", "{}", cancelled.detail);
    assert_eq!(cancelled.total_count, first.total_count);
    assert!(cancelled.processed_count < cancelled.total_count.unwrap());
    let partial_count = emls(resume_parent.path()).len();
    assert!(partial_count >= 3 && partial_count < files.len());
    // 取消时连接可能被丢弃；重新登录模拟重启后再次选择同一目录。
    let mut resumed_session = imap::login(&me, "pw").await.unwrap();
    let resumed = bulk_download::run_session(
        &mut resumed_session,
        &me,
        resume_parent.path(),
        false,
        Arc::default(),
        |_| {},
    )
    .await;
    assert_eq!(resumed.state, "complete", "{}", resumed.detail);
    assert_eq!(resumed.existing_count as usize, partial_count);
    assert_eq!(emls(resume_parent.path()).len(), files.len());

    fs::write(&files[0], b"existing local content must not be overwritten").unwrap();
    let conflict = bulk_download::run_session(
        &mut resumed_session,
        &me,
        parent.path(),
        false,
        Arc::default(),
        |_| {},
    )
    .await;
    assert_eq!(conflict.state, "error", "有冲突不能假报全部完成");
    assert!(conflict.failed_count > 0);
    assert_eq!(
        fs::read(&files[0]).unwrap(),
        b"existing local content must not be overwritten"
    );
    assert_eq!(emls(parent.path()).len(), files.len());
    println!(
        "全量导出通过：531封跨7个文件夹、同名子文件夹隔离、空文件夹、500封以上完整遍历、完整EML与附件、未读不变、忽略自动过滤、重复不增文件、取消后跨连接继续、冲突保留并报错"
    );
    let _ = resumed_session.logout().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn e2e_rich_reply_preserves_format_and_inline_image() {
    use futures::TryStreamExt;
    use mail_parser::{MessageParser, MimeHeaders};

    let me = account("me");
    let bob = account("bob");
    let mut original_mailbox = imap::login(&bob, "pw").await.unwrap();
    original_mailbox.select("INBOX").await.unwrap();
    original_mailbox
        .append("INBOX", None, None, smtp::RICH_REPLY_SOURCE.as_bytes())
        .await
        .unwrap();
    original_mailbox.noop().await.unwrap();
    let source_uid = *imap::page_uids(&mut original_mailbox, None)
        .await
        .unwrap()
        .last()
        .unwrap();
    let cache = std::sync::Mutex::new(crate::BodyCache::default());
    let (source_bytes, _, validity) = crate::read_raw(
        &mut original_mailbox,
        &cache,
        "bob",
        "INBOX",
        source_uid,
        true,
        None,
    )
    .await
    .unwrap();
    let source_raw = source_bytes.as_slice();
    assert!(
        crate::read_raw(
            &mut original_mailbox,
            &cache,
            "bob",
            "INBOX",
            source_uid,
            true,
            Some(validity.wrapping_add(1)),
        )
        .await
        .is_err(),
        "引用来源代际不一致必须拒绝，即使缓存存在"
    );
    let (quoted_bytes, _, _) = crate::read_raw(
        &mut original_mailbox,
        &cache,
        "bob",
        "INBOX",
        source_uid,
        true,
        Some(validity),
    )
    .await
    .unwrap();
    assert_eq!(quoted_bytes.as_slice(), source_raw);
    let source = imap::parse_message(source_raw, 0).unwrap();
    assert!(
        source
            .html
            .as_deref()
            .unwrap()
            .contains("data:image/png;base64,")
    );
    assert!(
        source
            .reply_html
            .as_deref()
            .unwrap()
            .contains("cid:logo@example.com")
    );

    // 发件账号与引用来源不同，仍必须保留源邮件中真正的图片。
    let html = format!(
        "<div>已收到，谢谢</div><blockquote>{}</blockquote>",
        source.reply_html.unwrap()
    );
    let draft: smtp::Draft = serde_json::from_value(serde_json::json!({
        "accountId":"me", "to":"bob@localhost", "subject":"Re: rich reply verification",
        "body":"已收到，谢谢\n\n> 原文格式", "htmlBody":html,
        "quoteSource":{"accountId":"bob", "folder":"INBOX", "uid":source_uid, "uidValidity":validity},
        "quoteCids":["logo@example.com"], "inReplyTo":"original-rich@example.com", "references":[]
    }))
    .unwrap();
    let (message, message_id) = smtp::build_with_quote(&me, &draft, Some(source_raw)).unwrap();
    let sent_raw = message.formatted();
    smtp::send(&me, "pw", message)
        .await
        .expect("只发送到本机隔离邮箱");
    original_mailbox.noop().await.unwrap();
    let mut received_uid = None;
    for _ in 0..20 {
        let found = original_mailbox
            .uid_search(format!("HEADER Message-ID \"{message_id}\""))
            .await
            .unwrap();
        received_uid = found.into_iter().next();
        if received_uid.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        original_mailbox.noop().await.unwrap();
    }
    let received: Vec<_> = original_mailbox
        .uid_fetch(received_uid.expect("收到回复").to_string(), "BODY.PEEK[]")
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let received_raw = received[0].body().unwrap();
    let received_mime = MessageParser::default().parse(received_raw).unwrap();
    let original_mime = MessageParser::default().parse(source_raw).unwrap();
    assert_eq!(received_mime.body_html(0).unwrap().trim(), html.trim());
    assert_eq!(
        received_mime
            .body_text(0)
            .unwrap()
            .trim()
            .replace("\r\n", "\n"),
        draft.body
    );
    assert_eq!(
        received_mime.in_reply_to().as_text(),
        Some("original-rich@example.com")
    );
    assert_eq!(received_mime.attachments().count(), 1);
    let image = received_mime.attachments().next().unwrap();
    assert_eq!(image.content_id(), Some("logo@example.com"));
    assert_eq!(
        image.contents(),
        original_mime
            .attachments()
            .find(|p| p.content_id() == Some("logo@example.com"))
            .unwrap()
            .contents()
    );
    assert!(
        !imap::fetch_flags(&mut original_mailbox, &source_uid.to_string())
            .await
            .unwrap()[0]
            .seen,
        "读取引用图片不得改变原邮件未读状态"
    );

    let mut sent_mailbox = imap::login(&me, "pw").await.unwrap();
    let _ = sent_mailbox.create("Sent").await;
    sent_mailbox.select("Sent").await.unwrap();
    assert!(
        imap::ensure_saved(&mut sent_mailbox, "Sent", &message_id, &sent_raw)
            .await
            .unwrap()
    );
    let saved_uid = *imap::page_uids(&mut sent_mailbox, None)
        .await
        .unwrap()
        .last()
        .unwrap();
    let saved: Vec<_> = sent_mailbox
        .uid_fetch(saved_uid.to_string(), "BODY.PEEK[]")
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let saved_mime = MessageParser::default()
        .parse(saved[0].body().unwrap())
        .unwrap();
    assert_eq!(saved_mime.body_html(0).unwrap().trim(), html.trim());
    assert_eq!(
        saved_mime.attachments().next().unwrap().contents(),
        image.contents()
    );
    println!(
        "HTML表格、文字样式、链接、图片原字节、纯文本fallback与回复头均经SMTP/IMAP回读验证，普通附件未复制，原邮件仍未读，已发送副本一致"
    );
    let _ = original_mailbox.logout().await;
    let _ = sent_mailbox.logout().await;
}

fn account(user: &str) -> Account {
    let spec = std::env::var("MAILPULSE_E2E").expect("需要 MAILPULSE_E2E=host:imap端口:smtp端口");
    let mut it = spec.split(':');
    let host = it.next().unwrap().to_string();
    Account {
        id: user.into(),
        kind: "custom".into(),
        email: format!("{user}@localhost"),
        name: format!("{user} 测试"),
        signature: String::new(),
        auto_download: Default::default(),
        receive_protocol: crate::config::ReceiveProtocol::Imap,
        pop3_host: String::new(),
        pop3_port: 995,
        pop3_tls: true,
        imap_host: host.clone(),
        imap_port: it.next().unwrap().parse().unwrap(),
        smtp_host: host,
        smtp_port: it.next().unwrap().parse().unwrap(),
    }
}

const HTML_WITH_ATTACHMENT: &str = "From: =?UTF-8?B?5byg5LiJ?= <zhang@example.com>\r\n\
To: me@localhost\r\n\
Subject: =?UTF-8?B?5ZGo5oql77ya56ys5LiJ5a2j5bqm?=\r\n\
Message-ID: <weekly-1@example.com>\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"outer\"\r\n\
\r\n\
--outer\r\n\
Content-Type: text/html; charset=utf-8\r\n\
\r\n\
<html><head><style>p{color:red}</style></head><body><p>\u{4f60}\u{597d}\u{ff0c}\u{8fd9}\u{662f}\u{5468}\u{62a5}\u{6b63}\u{6587}</p><img src=\"https://tracker.example.com/p.gif\"></body></html>\r\n\
--outer\r\n\
Content-Type: application/pdf; name=\"report.pdf\"\r\n\
Content-Disposition: attachment; filename=\"report.pdf\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
JVBERi0xLjQKJcfsj6IK\r\n\
--outer--\r\n";

/// 用单独的连接取摘要：解析失败会弄坏连接。
/// GreenMail 的部分 FETCH 回包写成 `BODY[]<0>{n}`（少了空格，不合 RFC 3501），解析必然失败，跳过。
async fn preview(acct: &Account, uid: u32) -> Option<String> {
    let mut s = imap::login(acct, "pw").await.unwrap();
    s.select("INBOX").await.unwrap();
    match imap::fetch_previews(&mut s, &uid.to_string()).await {
        Ok(v) => Some(v.into_iter().next().expect("应有摘要").text),
        Err(e) if format!("{e:#}").contains("BODY[]<0>{") => {
            println!("（服务器部分 FETCH 回包不合规范，跳过摘要检查）");
            None
        }
        Err(e) => panic!("取摘要失败：{e:#}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_full_flow() {
    let me = account("me");
    let bob = account("bob");

    // ---- 登录 + 文件夹 ----
    let mut s = imap::login(&me, "pw").await.expect("登录");
    for f in ["Sent", "Trash", "Archive"] {
        let _ = s.create(f).await;
    }
    let folders = imap::list_folders(&mut s).await.unwrap();
    let role = |r: &str| folders.iter().find(|f| f.role == r).map(|f| f.path.clone());
    println!(
        "文件夹：{:?}",
        folders
            .iter()
            .map(|f| (&f.path, &f.name, &f.role))
            .collect::<Vec<_>>()
    );
    assert_eq!(role("inbox").as_deref(), Some("INBOX"));
    assert_eq!(role("sent").as_deref(), Some("Sent"));
    assert_eq!(role("trash").as_deref(), Some("Trash"));

    // 密码错误要识别成 AuthFailed（同步任务据此停止重试）
    let bad = imap::login(&me, "wrong").await.expect_err("错误密码应失败");
    assert!(bad.is::<imap::AuthFailed>(), "{bad:#}");

    // ---- IDLE 推送延迟：bob 发信给 me，看 me 的 IDLE 多久被唤醒 ----
    assert!(
        imap::has_capability(&mut s, "IDLE").await.unwrap(),
        "服务器应支持 IDLE"
    );
    s.select("INBOX").await.unwrap();
    let before = imap::page_uids(&mut s, None).await.unwrap();
    let last = before.last().copied().unwrap_or(0);

    let mut idle = s.idle();
    idle.init().await.unwrap();
    let (wait, _stop) = idle.wait_with_timeout(Duration::from_secs(30));

    let draft = smtp::Draft {
        account_id: "bob".into(),
        to: "我 <me@localhost>".into(),
        cc: String::new(),
        bcc: String::new(),
        subject: "推送测试：你好 👋".into(),
        body: "第一行\n第二行 https://example.com/x".into(),
        html_body: None,
        quote_source: None,
        quote_cids: vec![],
        in_reply_to: None,
        references: vec![],
    };
    let (msg, mid) = smtp::build(&bob, &draft).unwrap();
    let raw_sent = msg.formatted();
    let t0 = Instant::now();
    smtp::send(&bob, "pw", msg).await.expect("SMTP 发送");
    let sent_at = t0.elapsed();
    let woke = wait.await.unwrap();
    let push = t0.elapsed();
    println!("SMTP 发送耗时 {sent_at:?}，IDLE 在发信开始后 {push:?} 被唤醒（{woke:?}）");
    assert!(
        matches!(woke, IdleResponse::NewData(_)),
        "应由新邮件唤醒，而不是超时"
    );
    assert!(push < Duration::from_secs(5));
    let mut s = idle.done().await.unwrap();

    // ---- 新邮件摘要 ----
    let fresh: Vec<_> = imap::fetch_summaries(&mut s, &format!("{}:*", last + 1), "me", "INBOX")
        .await
        .unwrap()
        .into_iter()
        .filter(|m| m.uid > last)
        .collect();
    assert_eq!(fresh.len(), 1);
    let m = &fresh[0];
    assert_eq!(m.subject, "推送测试：你好 👋");
    assert_eq!(m.from_name, "bob 测试");
    assert_eq!(m.from_addr, "bob@localhost");
    assert!(!m.seen);
    assert!(m.date > 0);
    if let Some(pv) = preview(&me, m.uid).await {
        assert_eq!(pv, "第一行 第二行 https://example.com/x");
    }
    assert!(imap::unseen_count(&mut s).await.unwrap() >= 1);

    // ---- HTML + 附件邮件（直接 APPEND 进收件箱）----
    s.append("INBOX", None, None, HTML_WITH_ATTACHMENT.as_bytes())
        .await
        .unwrap();
    s.noop().await.unwrap();
    let all = imap::page_uids(&mut s, None).await.unwrap();
    let html_uid = *all.last().unwrap();
    let sums = imap::fetch_summaries(&mut s, &html_uid.to_string(), "me", "INBOX")
        .await
        .unwrap();
    assert_eq!(sums[0].subject, "周报：第三季度");
    assert_eq!(sums[0].from_name, "张三");
    assert!(sums[0].attachment, "multipart/mixed 应显示附件图标");
    if let Some(pv) = preview(&me, html_uid).await {
        assert!(pv.contains("这是周报正文"), "HTML 摘要：{pv:?}");
        assert!(!pv.contains("color"), "摘要不应包含 CSS：{pv:?}");
    }

    // 打开邮件：BODY[] 会把它标为已读
    let f = s
        .uid_fetch(html_uid.to_string(), "(UID INTERNALDATE BODY[])")
        .await
        .unwrap();
    use futures::TryStreamExt;
    let f: Vec<_> = f.try_collect().await.unwrap();
    let raw = f[0].body().unwrap().to_vec();
    let v = imap::parse_message(&raw, 0).unwrap();
    assert!(v.html.as_deref().unwrap().contains("这是周报正文"));
    assert!(v.has_remote, "应识别出远程图片");
    assert_eq!(v.attachments.len(), 1);
    assert_eq!(v.attachments[0].name, "report.pdf");
    let (name, bytes) = imap::attachment_bytes(&raw, 0).unwrap();
    assert_eq!(name, "report.pdf");
    assert!(bytes.starts_with(b"%PDF-1.4"));
    let flags = imap::fetch_flags(&mut s, &html_uid.to_string())
        .await
        .unwrap();
    assert!(flags[0].seen, "打开后应为已读");

    // ---- 星标 / 未读 ----
    let _: Vec<_> = s
        .uid_store(m.uid.to_string(), "+FLAGS.SILENT (\\Flagged)")
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let flagged: Vec<u32> = s.uid_search("FLAGGED").await.unwrap().into_iter().collect();
    assert_eq!(flagged, vec![m.uid]);
    let _: Vec<_> = s
        .uid_store(html_uid.to_string(), "-FLAGS.SILENT (\\Seen)")
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    assert!(
        !imap::fetch_flags(&mut s, &html_uid.to_string())
            .await
            .unwrap()[0]
            .seen
    );

    // ---- 移到废纸篓，再从废纸篓彻底删除 ----
    imap::move_uids(&mut s, &html_uid.to_string(), Some("Trash"))
        .await
        .unwrap();
    assert!(
        !imap::page_uids(&mut s, None)
            .await
            .unwrap()
            .contains(&html_uid),
        "应已离开收件箱"
    );
    s.select("Trash").await.unwrap();
    let trash = imap::page_uids(&mut s, None).await.unwrap();
    assert_eq!(trash.len(), 1);
    imap::move_uids(&mut s, &trash[0].to_string(), None)
        .await
        .unwrap();
    assert!(
        imap::page_uids(&mut s, None).await.unwrap().is_empty(),
        "废纸篓里应已彻底删除"
    );

    // ---- 发信后补存到「已发送」，且不会重复保存 ----
    s.select("Sent").await.unwrap();
    assert!(
        imap::ensure_saved(&mut s, "Sent", &mid, &raw_sent)
            .await
            .unwrap(),
        "第一次应补存"
    );
    s.noop().await.unwrap();
    assert!(
        !imap::ensure_saved(&mut s, "Sent", &mid, &raw_sent)
            .await
            .unwrap(),
        "第二次应发现已存在"
    );
    let sent = imap::page_uids(&mut s, None).await.unwrap();
    let sums = imap::fetch_summaries(&mut s, &imap::uid_set(&sent), "me", "Sent")
        .await
        .unwrap();
    assert_eq!(sums.len(), 1);
    assert!(sums[0].seen);
    assert_eq!(sums[0].to, "我");

    // ---- 回复：In-Reply-To/References 正确，对方收到 ----
    let reply = smtp::Draft {
        account_id: "me".into(),
        to: "bob@localhost".into(),
        cc: String::new(),
        bcc: String::new(),
        subject: "Re: 推送测试".into(),
        body: "收到".into(),
        html_body: None,
        quote_source: None,
        quote_cids: vec![],
        in_reply_to: Some(mid.clone()),
        references: vec![],
    };
    let (msg, _) = smtp::build(&me, &reply).unwrap();
    smtp::send(&me, "pw", msg).await.unwrap();
    let mut b = imap::login(&bob, "pw").await.unwrap();
    b.select("INBOX").await.unwrap();
    let mut got = Vec::new();
    for _ in 0..20 {
        got = imap::page_uids(&mut b, None).await.unwrap();
        if !got.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        b.noop().await.unwrap();
    }
    let f: Vec<_> = b
        .uid_fetch(got.last().unwrap().to_string(), "BODY.PEEK[]")
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let v = imap::parse_message(f[0].body().unwrap(), 0).unwrap();
    assert_eq!(v.subject, "Re: 推送测试");
    assert_eq!(
        v.references,
        vec![mid.trim_matches(|c| c == '<' || c == '>').to_string()]
    );
    assert_eq!(v.text.trim(), "收到");

    let _ = timeout(OP_TIMEOUT, s.logout()).await;
    let _ = timeout(OP_TIMEOUT, b.logout()).await;
}

/// 在符合 RFC 的服务器（如 Dovecot）上验证摘要、文件夹识别和 IDLE。
/// MAILPULSE_E2E_IMAP=host:端口，账号 me@localhost / pass。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_imap_previews() {
    let spec = std::env::var("MAILPULSE_E2E_IMAP").expect("需要 MAILPULSE_E2E_IMAP=host:port");
    let (host, port) = spec.split_once(':').unwrap();
    let acct = Account {
        id: "dc".into(),
        kind: "custom".into(),
        email: "me@localhost".into(),
        name: "me".into(),
        signature: String::new(),
        auto_download: Default::default(),
        receive_protocol: crate::config::ReceiveProtocol::Imap,
        pop3_host: String::new(),
        pop3_port: 995,
        pop3_tls: true,
        imap_host: host.into(),
        imap_port: port.parse().unwrap(),
        smtp_host: String::new(),
        smtp_port: 465,
    };
    let mut s = imap::login(&acct, "pass").await.unwrap();
    // 中文文件夹名走修改版 UTF-7
    for f in ["Sent", "Trash", "&UXZO1mWHTvZZOQ-"] {
        let _ = s.create(f).await;
    }
    let folders = imap::list_folders(&mut s).await.unwrap();
    println!(
        "文件夹：{:?}",
        folders
            .iter()
            .map(|f| (&f.path, &f.name, &f.role))
            .collect::<Vec<_>>()
    );
    assert!(
        folders
            .iter()
            .any(|f| f.name == "其他文件夹" && f.role == "other")
    );
    assert!(folders.iter().any(|f| f.path == "Sent" && f.role == "sent"));

    // 另一条连接在 IDLE，这边 APPEND，看推送
    let mut watcher = imap::login(&acct, "pass").await.unwrap();
    watcher.select("INBOX").await.unwrap();
    let mut idle = watcher.idle();
    idle.init().await.unwrap();
    let (wait, _stop) = idle.wait_with_timeout(Duration::from_secs(20));

    let plain = "From: =?UTF-8?B?5byg5LiJ?= <z@example.com>\r\nSubject: plain\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n\u{4f60}\u{597d}\u{ff0c}\r\n\u{8fd9}\u{662f}\u{6b63}\u{6587}\r\n";
    let t0 = Instant::now();
    s.append("INBOX", None, None, plain.as_bytes())
        .await
        .unwrap();
    s.append("INBOX", None, None, HTML_WITH_ATTACHMENT.as_bytes())
        .await
        .unwrap();
    let woke = wait.await.unwrap();
    println!("APPEND 后 {:?} 收到推送：{woke:?}", t0.elapsed());
    assert!(matches!(woke, IdleResponse::NewData(_)));
    let _ = idle.done().await.unwrap();

    s.select("INBOX").await.unwrap();
    let uids = imap::page_uids(&mut s, None).await.unwrap();
    let last2 = &uids[uids.len() - 2..];
    let pv = imap::fetch_previews(&mut s, &imap::uid_set(last2))
        .await
        .expect("Dovecot 上摘要应能正常解析");
    println!(
        "摘要：{:?}",
        pv.iter().map(|p| (&p.uid, &p.text)).collect::<Vec<_>>()
    );
    let get = |u: u32| {
        pv.iter()
            .find(|p| p.uid == u)
            .map(|p| p.text.clone())
            .unwrap()
    };
    assert_eq!(get(last2[0]), "你好， 这是正文");
    assert!(get(last2[1]).contains("这是周报正文"));
    assert!(!get(last2[1]).contains("color"));
    let sums = imap::fetch_summaries(&mut s, &imap::uid_set(last2), "dc", "INBOX")
        .await
        .unwrap();
    assert_eq!(sums.len(), 2);
    assert!(sums.iter().all(|m| !m.seen), "取摘要不应把邮件标为已读");
}

/// 独立测试账号上的中文文件夹、完整原文下载与移动；不接触系统钥匙串。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn e2e_folder_create_move_export() {
    use futures::TryStreamExt;
    let acct = account("me");
    let mut s = imap::login(&acct, "pw").await.unwrap();
    let suffix = chrono::Utc::now().timestamp_nanos_opt().unwrap();
    let source_name = format!("MailBeacon源-{suffix}");
    let target_name = format!("资料 & 合作 📨-{suffix}");
    let (source, _) = imap::create_folder(&mut s, &source_name)
        .await
        .expect("创建来源文件夹");
    let (target, folders) = imap::create_folder(&mut s, &target_name)
        .await
        .expect("创建中文文件夹");
    assert_eq!(target.name, target_name);
    assert_eq!(imap::decode_mutf7(&target.path), target_name);
    assert!(folders.iter().any(|f| f.path == target.path));
    assert!(
        imap::create_folder(&mut s, &target_name).await.is_err(),
        "重复名称不能假成功"
    );
    s.append(&source.path, None, None, HTML_WITH_ATTACHMENT.as_bytes())
        .await
        .unwrap();
    s.append(
        &source.path,
        Some("(\\Deleted)"),
        None,
        b"Subject: unrelated deleted message\r\n\r\nkeep me\r\n",
    )
    .await
    .unwrap();
    s.select(&source.path).await.unwrap();
    let uids = imap::page_uids(&mut s, None).await.unwrap();
    assert_eq!(uids.len(), 2);
    let uid = uids[0];
    let unrelated = uids[1];
    assert!(!imap::fetch_flags(&mut s, &uid.to_string()).await.unwrap()[0].seen);
    let fetched: Vec<_> = s
        .uid_fetch(uid.to_string(), "(UID BODY.PEEK[])")
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let raw = fetched[0].body().unwrap().to_vec();
    assert_eq!(raw, HTML_WITH_ATTACHMENT.as_bytes());
    let dir = std::env::temp_dir().join(format!("mailpulse-e2e-export-{suffix}"));
    let saved = crate::export::save_eml(&dir, &raw, uid).unwrap();
    assert_eq!(std::fs::read(saved).unwrap(), raw);
    assert!(
        !imap::fetch_flags(&mut s, &uid.to_string()).await.unwrap()[0].seen,
        "下载原文不能改变未读状态"
    );
    imap::move_uids(&mut s, &uid.to_string(), Some(&target.path))
        .await
        .expect("移动到新文件夹");
    let remaining = imap::page_uids(&mut s, None).await.unwrap();
    assert_eq!(
        remaining,
        vec![unrelated],
        "不能清除其他已标记 Deleted 的邮件"
    );
    s.select(&target.path).await.unwrap();
    let moved = imap::page_uids(&mut s, None).await.unwrap();
    assert_eq!(moved.len(), 1);
    assert!(
        !imap::fetch_flags(&mut s, &moved[0].to_string())
            .await
            .unwrap()[0]
            .seen
    );
    let fetched: Vec<_> = s
        .uid_fetch(moved[0].to_string(), "BODY.PEEK[]")
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    assert_eq!(fetched[0].body().unwrap(), raw);
    s.select("INBOX").await.unwrap();
    s.delete(&source.path).await.unwrap();
    s.delete(&target.path).await.unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    let _ = s.logout().await;
}

/// 黑白名单对实际 MIME 邮件生效，跳过也持久化进度，重启不重复、不改未读。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn e2e_auto_download_keyword_filters() {
    use crate::{auto_download as download, config::Store};
    use std::{fs, sync::Mutex};

    fn mail_files(directory: &std::path::Path) -> Vec<std::path::PathBuf> {
        fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap())
            .filter(|entry| {
                entry.file_type().unwrap().is_file()
                    && entry
                        .path()
                        .extension()
                        .is_some_and(|extension| extension == "eml")
            })
            .map(|entry| entry.path())
            .collect()
    }

    async fn collect(
        session: &mut imap::Imap,
        store: &Mutex<Store>,
        id: &str,
        revision: u64,
    ) -> Vec<u32> {
        let mut checked = Vec::new();
        loop {
            let checkpoint = store
                .lock()
                .unwrap()
                .get(id)
                .unwrap()
                .auto_download
                .checkpoint
                .unwrap();
            let batch = download::next_batch(session, checkpoint).await.unwrap();
            for uid in batch.uids {
                let raw = download::raw_message(session, uid).await.unwrap().unwrap();
                assert!(
                    download::commit(
                        store,
                        id,
                        revision,
                        download::Checkpoint {
                            last_uid: uid,
                            ..checkpoint
                        },
                        Some((uid, &raw))
                    )
                    .unwrap()
                );
                checked.push(uid);
            }
            assert!(
                download::commit(
                    store,
                    id,
                    revision,
                    download::Checkpoint {
                        last_uid: batch.through,
                        ..checkpoint
                    },
                    None
                )
                .unwrap()
            );
            if !batch.more {
                break;
            }
        }
        checked
    }

    let me = account("me");
    let mut session = imap::login(&me, "pw").await.unwrap();
    let baseline = download::baseline(&mut session).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("config");
    let archive = dir.path().join("download");
    fs::create_dir(&archive).unwrap();
    let mut account = me.clone();
    account.auto_download = serde_json::from_value(serde_json::json!({
        "enabled": true, "directory": archive, "revision": 1,
        "checkpoint": baseline, "filterMode": "whitelist",
        "keywords": ["发票", "billing@example.test", "项目"]
    }))
    .unwrap();
    let mut config = Store::load(config_dir.clone()).unwrap();
    config.accounts.push(account);
    config.save().unwrap();
    let mut store = Mutex::new(config);
    let examples = [
        "From: sender@example.test\r\nSubject: =?UTF-8?B?5Y+R56Wo?=\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nInvoice attached\r\n--x\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=invoice.bin\r\nContent-Transfer-Encoding: base64\r\n\r\nAP8=\r\n--x--\r\n",
        "From: Billing <BILLING@EXAMPLE.TEST>\r\nSubject: Statement\r\n\r\nSender matches\r\n",
        "From: sender@example.test\r\nSubject: Weekly update\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>项&#30446;进展</p>\r\n",
        "From: sender@example.test\r\nSubject: Ordinary message\r\n\r\nNothing matches\r\n",
        "From: sender@example.test\r\nSubject: Attachment only\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nNothing matches\r\n--x\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Disposition: attachment; filename=invoice.txt\r\n\r\n发票 项目 billing@example.test\r\n--x--\r\n",
    ];
    for raw in examples {
        session
            .append("INBOX", None, None, raw.as_bytes())
            .await
            .unwrap();
    }
    let mut checked = collect(&mut session, &store, &me.id, 1).await;
    assert_eq!(checked.len(), 5);
    assert_eq!(
        mail_files(&archive).len(),
        3,
        "白名单只保存主题、发件人或正文命中的邮件"
    );
    for raw in &examples[..3] {
        assert!(
            mail_files(&archive)
                .iter()
                .any(|path| fs::read(path).unwrap() == raw.as_bytes()),
            "完整原文和附件字节保持不变"
        );
    }
    store = Mutex::new(Store::load(config_dir.clone()).unwrap());
    let saved = store
        .lock()
        .unwrap()
        .get(&me.id)
        .unwrap()
        .auto_download
        .clone();
    assert_eq!(saved.saved_count, 3);
    assert!(
        download::next_batch(&mut session, saved.checkpoint.unwrap())
            .await
            .unwrap()
            .uids
            .is_empty(),
        "重启后不能重新检查已过滤的邮件"
    );

    let mut settings = saved;
    settings.filter_mode = serde_json::from_str("\"blacklist\"").unwrap();
    settings.keywords = vec!["noise".into()];
    settings.revision = 2;
    settings.checkpoint = Some(download::baseline(&mut session).await.unwrap());
    store
        .lock()
        .unwrap()
        .set_auto_download(&me.id, settings)
        .unwrap();
    let blacklist_examples = [
        "From: sender@example.test\r\nSubject: NOISE advertisement\r\n\r\nExclude this\r\n",
        "From: sender@example.test\r\nSubject: Daily digest\r\n\r\nMonthly Noise digest\r\n",
        "From: sender@example.test\r\nSubject: Keep this\r\n\r\nOrdinary message\r\n",
    ];
    for raw in blacklist_examples {
        session
            .append("INBOX", None, None, raw.as_bytes())
            .await
            .unwrap();
    }
    checked.extend(collect(&mut session, &store, &me.id, 2).await);
    assert_eq!(mail_files(&archive).len(), 4, "黑名单只排除命中邮件");
    store = Mutex::new(Store::load(config_dir).unwrap());
    assert!(collect(&mut session, &store, &me.id, 2).await.is_empty());
    let mut settings = store
        .lock()
        .unwrap()
        .get(&me.id)
        .unwrap()
        .auto_download
        .clone();
    settings.filter_mode = serde_json::from_str("\"none\"").unwrap();
    settings.revision = 3;
    store
        .lock()
        .unwrap()
        .set_auto_download(&me.id, settings)
        .unwrap();
    session
        .append("INBOX", None, None, blacklist_examples[0].as_bytes())
        .await
        .unwrap();
    checked.extend(collect(&mut session, &store, &me.id, 3).await);
    assert_eq!(
        mail_files(&archive).len(),
        5,
        "关闭过滤后不再应用保留的关键词"
    );
    let unseen = session
        .uid_search(format!(
            "UID {}:{} UNSEEN",
            checked[0],
            checked.last().unwrap()
        ))
        .await
        .unwrap();
    assert_eq!(unseen.len(), checked.len(), "命中和跳过均不能改变未读状态");
    println!("白名单3/5、黑名单1/3、关闭过滤1/1；跳过进度跨重启保留，完整EML字节一致，9封均仍未读");
    let _ = session.logout().await;
}

/// 启用时跳过历史邮件；跨重启补齐超过一页的新邮件，失败重试不重复、不改未读。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_auto_download_new_mail_restart_and_retry() {
    use crate::{auto_download as download, config::Store};
    use std::{fs, sync::Mutex};

    fn mail_files(directory: &std::path::Path) -> Vec<std::path::PathBuf> {
        fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap())
            .filter(|entry| {
                entry.file_type().unwrap().is_file()
                    && entry
                        .path()
                        .extension()
                        .is_some_and(|extension| extension == "eml")
            })
            .map(|entry| entry.path())
            .collect()
    }
    let me = account("me");
    let mut session = imap::login(&me, "pw").await.unwrap();
    let tag = chrono::Utc::now().timestamp_nanos_opt().unwrap();
    let historical = format!("Subject: historical-{tag}\r\n\r\nnot automatically archived\r\n");
    session
        .append("INBOX", None, None, historical.as_bytes())
        .await
        .unwrap();
    let baseline = download::baseline(&mut session).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("config");
    let archive = dir.path().join("邮件下载");
    fs::create_dir(&archive).unwrap();
    let mut store = Store::load(config_dir.clone()).unwrap();
    let mut enabled = me.clone();
    enabled.auto_download = download::Settings {
        enabled: true,
        directory: download::validate_directory(archive.to_str().unwrap())
            .unwrap()
            .display()
            .to_string(),
        revision: 1,
        checkpoint: Some(baseline),
        ..Default::default()
    };
    store.accounts.push(enabled);
    store.save().unwrap();
    let batch = download::next_batch(&mut session, baseline).await.unwrap();
    assert!(batch.uids.is_empty(), "开启时不能下载历史邮件");

    let raw = |index| {
        format!(
            "From: bob@localhost\r\nTo: me@localhost\r\nSubject: automatic-{tag}-{index}\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n自动下载完整邮件 {index}\r\n--x\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=data.bin\r\nContent-Transfer-Encoding: base64\r\n\r\nAP8=\r\n--x--\r\n"
        )
    };
    for index in 0..125 {
        session
            .append("INBOX", None, None, raw(index).as_bytes())
            .await
            .unwrap();
    }
    let mut store = Mutex::new(Store::load(config_dir.clone()).unwrap());
    let batch = download::next_batch(&mut session, baseline).await.unwrap();
    assert_eq!(batch.uids.len(), 20, "下载按有界批次进行");
    let first = batch.uids[0];
    let bytes = download::raw_message(&mut session, first)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bytes, raw(0).as_bytes());
    let first_checkpoint = download::Checkpoint {
        last_uid: first,
        ..baseline
    };

    // 目录暂不可用时不越过失败邮件；恢复后自动重试。
    fs::rename(&archive, dir.path().join("disconnected")).unwrap();
    assert!(download::commit(&store, &me.id, 1, first_checkpoint, Some((first, &bytes))).is_err());
    assert_eq!(
        store
            .lock()
            .unwrap()
            .get(&me.id)
            .unwrap()
            .auto_download
            .checkpoint,
        Some(baseline)
    );
    fs::rename(dir.path().join("disconnected"), &archive).unwrap();

    // 文件已落盘而进度写入失败，模拟此时退出应用；重开后应复用同一文件。
    fs::create_dir(config_dir.join("accounts.json.tmp")).unwrap();
    assert!(download::commit(&store, &me.id, 1, first_checkpoint, Some((first, &bytes))).is_err());
    assert_eq!(mail_files(&archive).len(), 1);
    fs::remove_dir(config_dir.join("accounts.json.tmp")).unwrap();
    store = Mutex::new(Store::load(config_dir.clone()).unwrap());
    assert_eq!(
        store
            .lock()
            .unwrap()
            .get(&me.id)
            .unwrap()
            .auto_download
            .checkpoint,
        Some(baseline)
    );

    let mut seen_uids = Vec::new();
    loop {
        let checkpoint = store
            .lock()
            .unwrap()
            .get(&me.id)
            .unwrap()
            .auto_download
            .checkpoint
            .unwrap();
        let batch = download::next_batch(&mut session, checkpoint)
            .await
            .unwrap();
        assert!(batch.uids.len() <= 20);
        for uid in batch.uids {
            let bytes = download::raw_message(&mut session, uid)
                .await
                .unwrap()
                .unwrap();
            let next = download::Checkpoint {
                last_uid: uid,
                ..checkpoint
            };
            assert!(download::commit(&store, &me.id, 1, next, Some((uid, &bytes))).unwrap());
            seen_uids.push(uid);
        }
        let next = download::Checkpoint {
            last_uid: batch.through,
            ..checkpoint
        };
        assert!(download::commit(&store, &me.id, 1, next, None).unwrap());
        if !batch.more {
            break;
        }
    }
    assert_eq!(
        seen_uids.len(),
        125,
        "重开补齐所有新邮件，不受首屏100封限制"
    );
    assert_eq!(mail_files(&archive).len(), 125, "重试没有重复文件");
    let loaded = Store::load(config_dir).unwrap();
    let settings = &loaded.get(&me.id).unwrap().auto_download;
    assert_eq!(settings.saved_count, 125);
    let again = download::next_batch(&mut session, settings.checkpoint.unwrap())
        .await
        .unwrap();
    assert!(again.uids.is_empty(), "重启不重复下载");
    let unseen = session
        .uid_search(format!("UID {}:{} UNSEEN", seen_uids[0], seen_uids[124]))
        .await
        .unwrap();
    assert_eq!(unseen.len(), 125, "PEEK下载不能改变未读");
    let wrong = download::Checkpoint {
        uid_validity: baseline.uid_validity.wrapping_add(1),
        ..baseline
    };
    assert!(
        format!(
            "{:#}",
            download::next_batch(&mut session, wrong)
                .await
                .err()
                .unwrap()
        )
        .contains("关闭后重新开启")
    );
    for index in 0..125 {
        let expected = raw(index).into_bytes();
        assert!(
            mail_files(&archive)
                .iter()
                .any(|path| fs::read(path).unwrap() == expected)
        );
    }
    // SEARCH 与 FETCH 之间邮件在别处被删除：确认不存在后跳过，不阻塞后续邮件。
    let before_gap = settings.checkpoint.unwrap();
    session
        .append(
            "INBOX",
            None,
            None,
            b"Subject: gap\r\n\r\nremoved before download\r\n",
        )
        .await
        .unwrap();
    let pending = download::next_batch(&mut session, before_gap)
        .await
        .unwrap();
    let removed_uid = *pending.uids.last().unwrap();
    // 下载查询使用只读 EXAMINE；模拟客户端删除前必须重新以读写方式选择邮箱。
    session.select("INBOX").await.unwrap();
    imap::move_uids(&mut session, &removed_uid.to_string(), None)
        .await
        .unwrap();
    assert!(
        download::raw_message(&mut session, removed_uid)
            .await
            .unwrap()
            .is_none()
    );
    let gap = download::next_batch(&mut session, before_gap)
        .await
        .unwrap();
    assert!(gap.uids.is_empty());
    assert!(gap.through > before_gap.last_uid, "空 UID 窗口也推进进度");
    download::commit(
        &store,
        &me.id,
        1,
        download::Checkpoint {
            last_uid: gap.through,
            ..before_gap
        },
        None,
    )
    .unwrap();
    assert_eq!(
        store
            .lock()
            .unwrap()
            .get(&me.id)
            .unwrap()
            .auto_download
            .saved_count,
        125
    );
    let _ = session.logout().await;
    println!(
        "自动下载验收通过：跳过历史，125封跨重启补齐，目录恢复与写盘失败重试，原文和附件不变、未读不变、无重复文件、已删除 UID 不阻塞"
    );
}
