use dioxus_core::{ScopeId, VirtualDom};
use dioxus_signals::{GlobalKey, Writable};

pub use dioxus_devtools_types::*;
pub use subsecond;
use subsecond::PatchError;

/// Applies template and literal changes to the VirtualDom
///
/// Assets need to be handled by the renderer.
pub fn apply_changes(dom: &VirtualDom, msg: &HotReloadMsg) {
    try_apply_changes(dom, msg).unwrap()
}

/// Applies template and literal changes to the VirtualDom, but doesn't panic if patching fails.
///
/// Assets need to be handled by the renderer.
pub fn try_apply_changes(dom: &VirtualDom, msg: &HotReloadMsg) -> Result<(), PatchError> {
    dom.runtime().on_scope(ScopeId::ROOT, || {
        // 1. Update signals...
        let ctx = dioxus_signals::get_global_context();
        for template in &msg.templates {
            let value = template.template.clone();
            let key = GlobalKey::File {
                file: template.key.file.as_str(),
                line: template.key.line as _,
                column: template.key.column as _,
                index: template.key.index as _,
            };
            if let Some(mut signal) = ctx.get_signal_with_key(key.clone()) {
                signal.set(Some(value));
            }
        }

        // 2. Attempt to hotpatch
        if let Some(jump_table) = msg.jump_table.as_ref().cloned() {
            if msg.for_build_id == Some(dioxus_cli_config::build_id()) {
                unsafe { subsecond::apply_patch(jump_table) }?;
                dioxus_core::prelude::force_all_dirty();
            }
        }

        Ok(())
    })
}

/// Connect to the devserver and handle its messages with a callback.
///
/// This doesn't use any form of security or protocol, so it's not safe to expose to the internet.
#[cfg(not(target_arch = "wasm32"))]
pub fn connect(endpoint: String, mut callback: impl FnMut(DevserverMsg) + Send + 'static) {
    std::thread::spawn(move || {
        let uri = format!(
            "{endpoint}?aslr_reference={}&build_id={}",
            subsecond::__aslr_reference(),
            dioxus_cli_config::build_id()
        );

        let (mut websocket, _req) = match tungstenite::connect(uri) {
            Ok((websocket, req)) => (websocket, req),
            Err(_) => return,
        };

        while let Ok(msg) = websocket.read() {
            if let tungstenite::Message::Text(text) = msg {
                if let Ok(msg) = serde_json::from_str(&text) {
                    callback(msg);
                }
            }
        }
    });
}
