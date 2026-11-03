//! Ce qui relie un nœud à ses pairs (D48) : comment les joindre (`transport` : URL
//! directe ou tunnel SSH) et comment installer le daemon en service (`service`). Le lien
//! lui-même est dans `crate::peer`.

pub mod service;
pub mod transport;

/// Jeton d'une URL `…/?token=…`.
pub(crate) fn token_in(query: Option<&str>) -> Option<&str> {
    query?
        .split('&')
        .find_map(|pair| pair.strip_prefix("token="))
}
