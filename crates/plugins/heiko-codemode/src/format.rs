//! Scripts as the cards show them: laid out one statement a line
//! ([`luau`]), so a script the model wrote on one line reads as code. Only the cards see this; the
//! sandbox runs the source as it came, so its errors' line numbers stay
//! the model's.

use std::{
    cell::RefCell,
    collections::HashMap,
    hash::{Hash, Hasher},
    sync::Arc,
};

mod doc;
mod luau;

/// Sources [`formatted`] keeps, per thread, before it starts over.
const CACHED: usize = 128;

thread_local! {
    static CACHE: RefCell<HashMap<u64, Arc<str>>> =
        RefCell::new(HashMap::new());
}

/// `code` formatted, or as it is when it does not parse, such as a
/// script still streaming in. Kept, as cards draw it each frame.
pub fn formatted(code: &str) -> Arc<str> {
    let mut hasher = std::hash::DefaultHasher::new();
    code.hash(&mut hasher);
    let key = hasher.finish();
    if let Some(text) = CACHE.with_borrow(|cache| cache.get(&key).cloned()) {
        return text;
    }
    let text: Arc<str> = format(code).unwrap_or_else(|| code.into()).into();
    CACHE.with_borrow_mut(|cache| {
        if cache.len() >= CACHED {
            cache.clear();
        }
        cache.insert(key, text.clone());
    });
    text
}

fn format(code: &str) -> Option<String> {
    // A card is narrow.
    let text = luau::format(code, 80)?;
    Some(text.trim_end().to_owned())
}
