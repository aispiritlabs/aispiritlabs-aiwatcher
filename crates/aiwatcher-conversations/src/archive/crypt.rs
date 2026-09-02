//! Envelope encryption for archived content, and the keyring that rotates it.
//!
//! The object store is not the security boundary. RustFS holds prompts,
//! datasets, annotations and training runs beside this, its credentials are in
//! the same environment as everything else, and a backup of that bucket is a
//! file somebody can copy. Conversation content is the one thing in the bucket
//! that must not be readable from the bucket, so it is sealed before it gets
//! there.
//!
//! ```text
//! master key (32 bytes, from AIWATCHER_CONVERSATION_KEYS)
//!    │  HKDF-SHA256, per-object random salt, object path as info
//!    ▼
//! data key ──► AES-256-GCM(random 96-bit nonce, aad = key_id ‖ path)
//!                                                        │
//!                                                        ▼
//!                                        { v, key_id, salt, nonce, ciphertext }
//! ```
//!
//! Three decisions carry it.
//!
//! **A key per object, derived rather than stored.** The master key never
//! encrypts anything directly, so a nonce collision would need the same random
//! salt *and* the same random nonce. Storing wrapped data keys instead would be
//! the same security with an extra object per turn.
//!
//! **The object's path is authenticated.** It is HKDF `info` and it is the
//! AEAD's associated data, so a ciphertext copied from one turn's key to
//! another's does not open. Without it, anyone who can write to the bucket can
//! substitute one person's words for another's and every digest still checks
//! out, because the digest is of the plaintext they never had to touch.
//!
//! **Old keys stay readable.** The keyring is ordered, the first entry seals,
//! and every entry opens. Rotation is prepending a key and re-deploying;
//! retiring one is removing it, which makes anything still sealed under it
//! unreadable — deliberately, because that is also how a key is destroyed on
//! purpose.

use std::collections::BTreeMap;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use ring::hkdf::{HKDF_SHA256, Salt};
use ring::rand::SecureRandom as _;
use serde::{Deserialize, Serialize};

/// Bytes a master key must have.
pub const KEY_BYTES: usize = 32;
const SALT_BYTES: usize = 32;
/// Bumped only if the construction changes. A sealed object records the version
/// it was written under, so a change of scheme can still read the old ones.
const SCHEME: u8 = 1;

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum KeyringError {
    #[error(
        "{name} is not a keyring: expected `id:key[,id:key]` where each key is {KEY_BYTES} \
         base64url-encoded bytes"
    )]
    Malformed { name: &'static str },

    #[error("{name} declares the key id {id:?} twice")]
    Duplicate { name: &'static str, id: String },

    #[error("{name} is empty; a conversation archive cannot run without a key")]
    Empty { name: &'static str },

    /// The object was sealed under a key this deployment no longer holds.
    /// Answering anything else — an empty body, a 404 — would make a retired
    /// key indistinguishable from content that was never there.
    #[error("this content was sealed under the key {id:?}, which this deployment does not hold")]
    UnknownKey { id: String },

    /// The ciphertext did not authenticate. Either it was altered, or it was
    /// moved to a different object path.
    #[error("the sealed content did not authenticate; it was altered or moved")]
    Tampered,

    #[error("the archive's random source failed")]
    Random,

    #[error("the sealed object is malformed: {0}")]
    Corrupt(String),
}

/// One sealed blob, as it sits in the object store.
///
/// JSON rather than a packed binary frame: every other object in this bucket is
/// a JSON document, and the 33% base64 overhead buys a store somebody can
/// inspect with `cat` when they are trying to work out what went wrong at
/// three in the morning. What they will see is the key id, the salt and the
/// nonce — none of which is a secret — and no plaintext.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SealedObject {
    /// The scheme this was written under.
    pub v: u8,
    pub key_id: String,
    pub salt: String,
    pub nonce: String,
    pub ciphertext: String,
}

/// The keys this deployment can seal and open with.
///
/// `Debug` is hand-written: the derived one would print the key material into
/// the first log line that formats the configuration.
#[derive(Clone)]
pub struct Keyring {
    active: String,
    keys: BTreeMap<String, [u8; KEY_BYTES]>,
}

impl std::fmt::Debug for Keyring {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keyring")
            .field("active", &self.active)
            .field("keys", &self.keys.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Keyring {
    /// Parse `id:key[,id:key]`, active key first.
    ///
    /// # Errors
    ///
    /// [`KeyringError::Malformed`] for anything that is not that shape or whose
    /// key is not exactly [`KEY_BYTES`] bytes, [`KeyringError::Duplicate`] for
    /// a repeated id, and [`KeyringError::Empty`] for a blank value.
    pub fn parse(name: &'static str, spec: &str) -> Result<Self, KeyringError> {
        let mut keys = BTreeMap::new();
        let mut active = None;
        for entry in spec.split(',').map(str::trim).filter(|e| !e.is_empty()) {
            let (id, encoded) = entry
                .split_once(':')
                .ok_or(KeyringError::Malformed { name })?;
            let id = id.trim();
            if id.is_empty() || id.len() > 64 || !id.bytes().all(is_id_byte) {
                return Err(KeyringError::Malformed { name });
            }
            let decoded = decode(encoded.trim()).ok_or(KeyringError::Malformed { name })?;
            let key: [u8; KEY_BYTES] = decoded
                .try_into()
                .map_err(|_| KeyringError::Malformed { name })?;
            if keys.insert(id.to_owned(), key).is_some() {
                return Err(KeyringError::Duplicate {
                    name,
                    id: id.to_owned(),
                });
            }
            active.get_or_insert_with(|| id.to_owned());
        }
        let active = active.ok_or(KeyringError::Empty { name })?;
        Ok(Self { active, keys })
    }

    /// A keyring holding one key, for tests and for a development instance that
    /// generates one at start-up.
    #[must_use]
    pub fn single(id: impl Into<String>, key: [u8; KEY_BYTES]) -> Self {
        let id = id.into();
        let mut keys = BTreeMap::new();
        keys.insert(id.clone(), key);
        Self { active: id, keys }
    }

    /// A fresh random master key, base64url-encoded for
    /// `AIWATCHER_CONVERSATION_KEYS`.
    ///
    /// # Errors
    ///
    /// [`KeyringError::Random`] when the system's random source fails.
    pub fn generate_key() -> Result<String, KeyringError> {
        let mut key = [0u8; KEY_BYTES];
        ring::rand::SystemRandom::new()
            .fill(&mut key)
            .map_err(|_| KeyringError::Random)?;
        Ok(URL_SAFE_NO_PAD.encode(key))
    }

    /// The id new objects are sealed under.
    #[must_use]
    pub fn active(&self) -> &str {
        &self.active
    }

    /// Every id this deployment can open, for a health report. Never the keys.
    #[must_use]
    pub fn key_ids(&self) -> Vec<&str> {
        self.keys.keys().map(String::as_str).collect()
    }

    /// Seal `plaintext` for the object at `path`.
    ///
    /// # Errors
    ///
    /// [`KeyringError::Random`] when the random source fails, and
    /// [`KeyringError::Tampered`] if the AEAD itself refuses — which in
    /// practice means a plaintext larger than the algorithm's limit.
    pub fn seal(&self, path: &str, plaintext: &[u8]) -> Result<SealedObject, KeyringError> {
        let random = ring::rand::SystemRandom::new();
        let mut salt = [0u8; SALT_BYTES];
        let mut nonce = [0u8; NONCE_LEN];
        random.fill(&mut salt).map_err(|_| KeyringError::Random)?;
        random.fill(&mut nonce).map_err(|_| KeyringError::Random)?;

        let master = self
            .keys
            .get(&self.active)
            .ok_or_else(|| KeyringError::UnknownKey {
                id: self.active.clone(),
            })?;
        let key = derive(master, &salt, path)?;

        let mut in_out = plaintext.to_vec();
        key.seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(aad(&self.active, path)),
            &mut in_out,
        )
        .map_err(|_| KeyringError::Tampered)?;

        Ok(SealedObject {
            v: SCHEME,
            key_id: self.active.clone(),
            salt: URL_SAFE_NO_PAD.encode(salt),
            nonce: URL_SAFE_NO_PAD.encode(nonce),
            ciphertext: URL_SAFE_NO_PAD.encode(&in_out),
        })
    }

    /// Open an object that was sealed for `path`.
    ///
    /// # Errors
    ///
    /// [`KeyringError::UnknownKey`] when the sealing key has been retired,
    /// [`KeyringError::Tampered`] when the ciphertext does not authenticate —
    /// which includes the case of a ciphertext copied from another object's
    /// path — and [`KeyringError::Corrupt`] for a document that is not a
    /// sealed object at all.
    pub fn open(&self, path: &str, sealed: &SealedObject) -> Result<Vec<u8>, KeyringError> {
        if sealed.v != SCHEME {
            return Err(KeyringError::Corrupt(format!(
                "sealed under scheme {}, and this build writes and reads {SCHEME}",
                sealed.v
            )));
        }
        let master = self
            .keys
            .get(&sealed.key_id)
            .ok_or_else(|| KeyringError::UnknownKey {
                id: sealed.key_id.clone(),
            })?;
        let salt = decode(&sealed.salt)
            .ok_or_else(|| KeyringError::Corrupt("the salt is not base64url".to_owned()))?;
        let nonce: [u8; NONCE_LEN] = decode(&sealed.nonce)
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or_else(|| KeyringError::Corrupt("the nonce is the wrong size".to_owned()))?;
        let mut in_out = decode(&sealed.ciphertext)
            .ok_or_else(|| KeyringError::Corrupt("the ciphertext is not base64url".to_owned()))?;

        let key = derive(master, &salt, path)?;
        let opened = key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(aad(&sealed.key_id, path)),
                &mut in_out,
            )
            .map_err(|_| KeyringError::Tampered)?;
        Ok(opened.to_vec())
    }
}

fn is_id_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
}

fn decode(value: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD
        .decode(value)
        .ok()
        // Accept the padded alphabet too: a key pasted out of a secret manager
        // very often carries `=`, and refusing it would be a start-up failure
        // whose message is about base64.
        .or_else(|| base64::engine::general_purpose::STANDARD.decode(value).ok())
        .or_else(|| base64::engine::general_purpose::URL_SAFE.decode(value).ok())
}

/// What the AEAD authenticates alongside the ciphertext.
fn aad(key_id: &str, path: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(key_id.len() + path.len() + 32);
    bytes.extend_from_slice(b"aiwatcher-conversation-archive/v1\0");
    bytes.extend_from_slice(key_id.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(path.as_bytes());
    bytes
}

fn derive(master: &[u8; KEY_BYTES], salt: &[u8], path: &str) -> Result<LessSafeKey, KeyringError> {
    let prk = Salt::new(HKDF_SHA256, salt).extract(master);
    let info = [b"content\0".as_slice(), path.as_bytes()];
    let okm = prk
        .expand(&info, &AES_256_GCM)
        .map_err(|_| KeyringError::Tampered)?;
    Ok(LessSafeKey::new(UnboundKey::from(okm)))
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn keyring() -> Keyring {
        Keyring::single("k1", [7u8; KEY_BYTES])
    }

    #[test]
    fn what_was_sealed_comes_back() {
        let ring = keyring();
        let sealed = ring
            .seal("conversations/turns/abc", b"the customer said hello")
            .expect("seals");
        let opened = ring
            .open("conversations/turns/abc", &sealed)
            .expect("opens");
        assert_eq!(opened, b"the customer said hello");
    }

    #[test]
    fn a_sealed_object_holds_no_plaintext() {
        let sealed = keyring()
            .seal("conversations/turns/abc", b"ada@example.com")
            .expect("seals");
        let json = serde_json::to_string(&sealed).expect("serialises");
        assert!(!json.contains("ada"), "{json}");
        assert!(!json.contains("example.com"), "{json}");
    }

    #[test]
    fn ciphertext_moved_to_another_objects_path_does_not_open() {
        // The substitution attack this defends against: anyone who can write
        // to the bucket copies one turn's content over another's. Both are
        // valid ciphertexts under the same master key, and without the path in
        // the associated data the swap is undetectable — the plaintext digest
        // checks out, because it is a digest of a real message, just not that
        // one.
        let ring = keyring();
        let sealed = ring
            .seal("conversations/turns/aaa", b"secret")
            .expect("seals");
        assert_eq!(
            ring.open("conversations/turns/bbb", &sealed),
            Err(KeyringError::Tampered)
        );
    }

    #[test]
    fn an_altered_ciphertext_does_not_open() {
        let ring = keyring();
        let mut sealed = ring.seal("path", b"secret").expect("seals");
        let mut bytes = decode(&sealed.ciphertext).expect("decodes");
        bytes[0] ^= 0xff;
        sealed.ciphertext = URL_SAFE_NO_PAD.encode(&bytes);
        assert_eq!(ring.open("path", &sealed), Err(KeyringError::Tampered));
    }

    #[test]
    fn two_seals_of_one_message_differ() {
        // A per-object random salt and nonce, so an observer of the bucket
        // cannot tell that two turns say the same thing.
        let ring = keyring();
        let first = ring.seal("path", b"same").expect("seals");
        let second = ring.seal("path", b"same").expect("seals");
        assert_ne!(first.ciphertext, second.ciphertext);
        assert_ne!(first.salt, second.salt);
    }

    #[test]
    fn a_rotated_key_still_opens_what_the_old_one_sealed() {
        let old = Keyring::single("2026-01", [1u8; KEY_BYTES]);
        let sealed = old.seal("path", b"kept").expect("seals");

        let mut keys = BTreeMap::new();
        keys.insert("2026-09".to_owned(), [2u8; KEY_BYTES]);
        keys.insert("2026-01".to_owned(), [1u8; KEY_BYTES]);
        let rotated = Keyring {
            active: "2026-09".to_owned(),
            keys,
        };
        assert_eq!(rotated.open("path", &sealed).expect("opens"), b"kept");
        // And a new seal uses the new key.
        assert_eq!(rotated.seal("path", b"x").expect("seals").key_id, "2026-09");
    }

    #[test]
    fn a_retired_key_says_so_rather_than_looking_like_missing_content() {
        let sealed = Keyring::single("gone", [3u8; KEY_BYTES])
            .seal("path", b"x")
            .expect("seals");
        assert_eq!(
            keyring().open("path", &sealed),
            Err(KeyringError::UnknownKey {
                id: "gone".to_owned()
            })
        );
    }

    #[test]
    fn the_first_key_in_the_spec_is_the_one_that_seals() {
        let key = URL_SAFE_NO_PAD.encode([9u8; KEY_BYTES]);
        let other = URL_SAFE_NO_PAD.encode([8u8; KEY_BYTES]);
        let ring = Keyring::parse(
            "AIWATCHER_CONVERSATION_KEYS",
            &format!("new:{key},old:{other}"),
        )
        .expect("parses");
        assert_eq!(ring.active(), "new");
        assert_eq!(ring.key_ids(), vec!["new", "old"]);
    }

    #[test]
    fn a_key_of_the_wrong_length_is_a_start_up_failure_rather_than_a_weak_key() {
        let short = URL_SAFE_NO_PAD.encode([1u8; 16]);
        assert!(matches!(
            Keyring::parse("K", &format!("k:{short}")),
            Err(KeyringError::Malformed { .. })
        ));
        assert!(matches!(
            Keyring::parse("K", ""),
            Err(KeyringError::Empty { .. })
        ));
        assert!(matches!(
            Keyring::parse("K", "no-colon"),
            Err(KeyringError::Malformed { .. })
        ));
    }

    #[test]
    fn a_repeated_key_id_is_refused_rather_than_silently_taking_the_last() {
        let key = URL_SAFE_NO_PAD.encode([1u8; KEY_BYTES]);
        assert!(matches!(
            Keyring::parse("K", &format!("k:{key},k:{key}")),
            Err(KeyringError::Duplicate { .. })
        ));
    }

    #[test]
    fn a_generated_key_is_usable_as_it_is_printed() {
        let encoded = Keyring::generate_key().expect("generates");
        let ring = Keyring::parse("K", &format!("dev:{encoded}")).expect("parses");
        let sealed = ring.seal("path", b"x").expect("seals");
        assert_eq!(ring.open("path", &sealed).expect("opens"), b"x");
    }

    #[test]
    fn a_key_pasted_with_padding_is_still_accepted() {
        // What comes out of a secret manager, most of the time.
        let padded = base64::engine::general_purpose::STANDARD.encode([4u8; KEY_BYTES]);
        assert!(Keyring::parse("K", &format!("k:{padded}")).is_ok());
    }

    #[test]
    fn a_debug_line_never_prints_key_material() {
        let printed = format!("{:?}", keyring());
        assert!(printed.contains("k1"), "{printed}");
        assert!(!printed.contains('7'), "{printed}");
    }
}
