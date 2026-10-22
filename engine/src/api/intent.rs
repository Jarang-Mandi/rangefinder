//! Generasi permintaan mencegah hasil RPC lama mengganti niat user yang baru.
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
pub struct Intent {
    revision: AtomicU64,
    lookup: AtomicU64,
    action: AtomicU64,
    hook: std::sync::Mutex<std::collections::HashMap<String, u64>>,
}

impl Intent {
    pub fn begin_hook(&self, address: &str) -> u64 {
        let mut hooks = self.hook.lock().expect("mutex generasi hook");
        let ticket = hooks.entry(address.to_lowercase()).or_default();
        *ticket += 1;
        *ticket
    }
    pub fn hook_current(&self, address: &str, ticket: u64) -> bool {
        self.hook
            .lock()
            .expect("mutex generasi hook")
            .get(&address.to_lowercase())
            .is_some_and(|current| *current == ticket)
    }
    pub fn begin_action(&self) -> u64 {
        self.action.fetch_add(1, Ordering::SeqCst) + 1
    }
    pub fn action_current(&self, ticket: u64) -> bool {
        self.action.load(Ordering::SeqCst) == ticket
    }
    pub fn change(&self) -> u64 {
        self.revision.fetch_add(1, Ordering::SeqCst) + 1
    }
    pub fn current(&self, revision: u64) -> bool {
        self.revision.load(Ordering::SeqCst) == revision
    }
    pub fn advance(&self, revision: u64) -> Option<u64> {
        self.revision
            .compare_exchange(revision, revision + 1, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| revision + 1)
    }
    pub fn begin_lookup(&self) -> u64 {
        self.change();
        self.lookup.fetch_add(1, Ordering::SeqCst) + 1
    }
    pub fn lookup_current(&self, ticket: u64) -> bool {
        self.lookup.load(Ordering::SeqCst) == ticket
    }
}

pub fn plan_id() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| format!("gagal membuat pengenal rencana: {e}"))?;
    Ok(hex::encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pencabutan_mengalahkan_penilaian_lama_tanpa_membatalkan_hook_lain() {
        let intent = Intent::default();
        let approve_a = intent.begin_hook("0xAAA");
        let approve_b = intent.begin_hook("0xBBB");
        assert!(intent.hook_current("0xaaa", approve_a));
        let revoke_a = intent.begin_hook("0xaaa");
        assert!(!intent.hook_current("0xAAA", approve_a));
        assert!(intent.hook_current("0xaaa", revoke_a));
        assert!(intent.hook_current("0xbbb", approve_b));
    }
    #[test]
    fn quote_lama_dan_hasil_lookup_terlambat_tidak_boleh_berlaku() {
        let i = Intent::default();
        let token_a = i.begin_lookup();
        let quote_a = i.change();
        let quote_b = i.change();
        assert!(!i.current(quote_a));
        assert!(i.current(quote_b));
        assert!(i.lookup_current(token_a));
        let token_b = i.begin_lookup();
        assert!(!i.lookup_current(token_a));
        assert!(!i.current(quote_b));
        assert!(i.lookup_current(token_b));
    }
}
