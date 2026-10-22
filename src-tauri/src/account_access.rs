//! 账户配置切换与邮件写操作共用的门禁。
use crate::config::Account;

impl crate::AppState {
    pub(crate) fn account_gate(&self, id: &str) -> std::sync::Arc<tokio::sync::RwLock<()>> {
        self.account_gates.lock().unwrap().entry(id.to_string()).or_default().clone()
    }

    pub(crate) fn connection_is_current(&self, account: &Account) -> bool {
        self.store.lock().unwrap().get(&account.id).is_some_and(|current| current.connection_revision == account.connection_revision && current.receive_epoch == account.receive_epoch)
    }

}
