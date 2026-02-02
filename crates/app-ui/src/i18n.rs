#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use leptos::prelude::{use_context, RwSignal, StoredValue, WithUntracked, WithValue};

use mf2_i18n_core::{Args, MessageId};
use mf2_i18n_embedded::{EmbeddedPack, EmbeddedRuntime};

#[derive(Clone, Copy)]
pub struct GittreeAppI18nContext {
    pub locale: RwSignal<String>,
    pub runtime: StoredValue<Option<EmbeddedRuntime>>,
}

pub fn app_i18n_init() -> GittreeAppI18nContext {
    let locale = RwSignal::new("en".to_string());
    let runtime = StoredValue::new(load_embedded_runtime());
    GittreeAppI18nContext { locale, runtime }
}

pub fn app_i18n() -> Option<GittreeAppI18nContext> {
    use_context::<GittreeAppI18nContext>()
}

pub fn translate(key: &str) -> String {
    let Some(ctx) = app_i18n() else {
        return key.to_string();
    };
    let locale = ctx.locale.with_untracked(|value| value.clone());
    ctx.runtime.with_value(|runtime: &Option<EmbeddedRuntime>| {
        if let Some(runtime) = runtime.as_ref() {
            let args = Args::new();
            runtime
                .format(&locale, key, &args)
                .unwrap_or_else(|_| key.to_string())
        } else {
            key.to_string()
        }
    })
}

fn load_embedded_runtime() -> Option<EmbeddedRuntime> {
    let id_map = load_id_map()?;
    let id_map_hash = load_id_map_hash()?;
    let packs = [EmbeddedPack {
        locale: "en",
        bytes: load_pack_en(),
    }];
    EmbeddedRuntime::new(id_map, id_map_hash, &packs, "en").ok()
}

fn load_id_map() -> Option<BTreeMap<String, MessageId>> {
    let raw = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/i18n/build/id_map.json"));
    let parsed: BTreeMap<String, u32> = serde_json::from_slice(raw).ok()?;
    let mut map = BTreeMap::new();
    for (key, id) in parsed {
        map.insert(key, MessageId::new(id));
    }
    Some(map)
}

fn load_id_map_hash() -> Option<[u8; 32]> {
    let raw = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/i18n/build/id_map_hash"));
    let text = std::str::from_utf8(raw).ok()?;
    let value = text.trim();
    let hex_value = value.strip_prefix("sha256:").unwrap_or(value);
    let bytes = hex::decode(hex_value).ok()?;
    if bytes.len() != 32 {
        return None;
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Some(out)
}

fn load_pack_en() -> &'static [u8] {
    include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/i18n/build/packs/en.mf2pack"
    ))
}

#[macro_export]
macro_rules! t {
    ($key:literal) => {
        $crate::i18n::translate($key)
    };
}

#[cfg(test)]
mod tests {
    use super::{app_i18n, app_i18n_init, translate};
    use leptos::prelude::{provide_context, Owner};

    #[test]
    fn translate_falls_back_without_context() {
        assert_eq!(translate("missing.key"), "missing.key");
    }

    #[test]
    fn translate_reads_context() {
        let owner = Owner::new();
        owner.set();
        provide_context(app_i18n_init());
        assert!(app_i18n().is_some());
        assert_eq!(translate("missing.key"), "missing.key");
    }
}
