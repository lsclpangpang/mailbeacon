//! Explicitly opted-in, isolated diagnostic for the production QQ IMAP login.
//! The private JSON path comes from MAILBEACON_QQ_LOGIN_FILE. Its only fields are
//! email and password. Run this test alone with --ignored --exact --nocapture.
//! No account is saved, no mailbox is selected, and no mail is fetched or sent.

use std::{panic::AssertUnwindSafe, time::Duration};

use futures::FutureExt;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Credentials {
    email: String,
    password: String,
}

fn error_category(error: &anyhow::Error) -> &'static str {
    if error.is::<crate::imap::AuthFailed>() {
        "authentication"
    } else if error.is::<tokio::time::error::Elapsed>() {
        "timeout"
    } else if error.is::<tokio_rustls::rustls::Error>() {
        "tls"
    } else if error.is::<std::io::Error>() {
        "io"
    } else {
        "other"
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires explicitly authorized private QQ credentials; run alone"]
async fn qq_login_diagnostic() {
    let credentials = std::env::var_os("MAILBEACON_QQ_LOGIN_FILE")
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice::<Credentials>(&bytes).ok())
        .expect("private diagnostic input is missing or invalid");
    let input = crate::config::NewAccount {
        kind: "qq".into(),
        email: credentials.email,
        password: credentials.password,
        name: "diagnostic".into(),
        receive_protocol: Some(crate::config::ReceiveProtocol::Imap),
        pop3_host: None,
        pop3_port: None,
        pop3_tls: None,
        imap_host: None,
        imap_port: None,
        smtp_host: None,
        smtp_port: None,
    };
    let (account, password) = input
        .into_account("diagnostic-not-persisted".into())
        .unwrap_or_else(|_| panic!("private diagnostic input is invalid"));

    // Dependencies may include a server response in a panic payload. This test
    // is deliberately run alone, and the original hook is restored afterwards.
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| eprintln!("error_category=panic")));
    let result = AssertUnwindSafe(async {
        eprintln!("before_login");
        // Production login already performs CAPABILITY and, when offered, ID.
        let mut session = crate::imap::login(&account, &password).await?;
        eprintln!("after_login");
        tokio::time::timeout(Duration::from_secs(5), session.logout()).await??;
        eprintln!("after_logout");
        Ok::<(), anyhow::Error>(())
    })
    .catch_unwind()
    .await;
    std::panic::set_hook(previous_hook);

    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            eprintln!("error_category={}", error_category(&error));
            panic!("QQ login diagnostic failed; response details redacted");
        }
        Err(_) => panic!("QQ login diagnostic panicked; payload redacted"),
    }
}

#[test]
fn error_categories_do_not_include_server_or_credential_text() {
    let secret_response = "sensitive server response";
    assert_eq!(
        error_category(&anyhow::Error::new(crate::imap::AuthFailed(
            secret_response.into()
        ))),
        "authentication"
    );
    assert_eq!(
        error_category(&anyhow::Error::new(std::io::Error::other(secret_response))),
        "io"
    );
    assert_eq!(error_category(&anyhow::anyhow!(secret_response)), "other");
}
