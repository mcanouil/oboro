//! The reversible mapping between real values and placeholders.
//!
//! The vault is the only part of `oboro` that holds sensitive data at rest, so
//! it is encrypted: values are sealed with AES-256-GCM under a key derived
//! from a local key file, and looked up through a keyed hash rather than the
//! plaintext. Possession of the database alone therefore reveals neither the
//! values nor whether a guessed value is present.

use std::path::{Path, PathBuf};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use anyhow::{Context, Result, anyhow};
use rand::Rng;
use rusqlite::{Connection, OptionalExtension, params};
use zeroize::Zeroizing;

use crate::detect::EntityKind;

/// Domain separation for the two keys derived from the master key.
const ENCRYPTION_CONTEXT: &str = "oboro 2026-07-19 vault value encryption";
const INDEX_CONTEXT: &str = "oboro 2026-07-19 vault value index";

const NONCE_LEN: usize = 12;

/// A stored mapping, as shown by `oboro map list`.
///
/// The tag and sequence are kept apart rather than rendered into a
/// placeholder, so a caller that needs to look the value up does not have to
/// parse back what this struct just formatted.
pub struct Entry {
    pub tag: String,
    pub seq: i64,
    pub created_at: String,
}

impl Entry {
    /// The placeholder this mapping issues, such as `[[PERSON_1]]`.
    #[must_use]
    pub fn placeholder(&self) -> String {
        placeholder(&self.tag, self.seq)
    }
}

/// An encrypted, deterministic placeholder store.
///
/// Deliberately not `Debug`: the derived implementation would print the
/// derived key material through `Zeroizing`.
pub struct Vault {
    connection: Connection,
    /// Built once: deriving the key schedule per operation is pure waste,
    /// and the key material it holds is already resident for the vault's life.
    cipher: Aes256Gcm,
    index_key: Zeroizing<[u8; 32]>,
}

impl Vault {
    /// Opens the vault at `db_path`, creating it and the key at `key_path` on
    /// first use.
    ///
    /// # Errors
    ///
    /// Returns an error if the directories cannot be created, the key file is
    /// not exactly 32 bytes, or the database cannot be opened.
    pub fn open(db_path: &Path, key_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            create_private_dir(parent)?;
        }
        let master = load_or_create_key(key_path)?;
        let connection = Connection::open(db_path)
            .with_context(|| format!("opening vault at {}", db_path.display()))?;
        restrict_permissions(db_path)?;
        initialise_schema(&connection)?;
        // WAL mode creates -wal and -shm sidecars next to the database; left to
        // the umask they would be world-readable, so tighten them to match.
        restrict_sidecars(db_path)?;

        Ok(Self {
            connection,
            cipher: Aes256Gcm::new(&Key::<Aes256Gcm>::from(blake3::derive_key(
                ENCRYPTION_CONTEXT,
                &*master,
            ))),
            index_key: Zeroizing::new(blake3::derive_key(INDEX_CONTEXT, &*master)),
        })
    }

    /// Returns the placeholder for `value`, allocating one on first sight.
    ///
    /// Allocation is deterministic within a vault: the same value always maps
    /// to the same placeholder, which keeps cross-document references
    /// coherent for whichever model reads the sanitised output.
    ///
    /// # Errors
    ///
    /// Returns an error if the value cannot be encrypted or the database
    /// rejects the write.
    pub fn placeholder_for(&mut self, kind: &EntityKind, value: &str) -> Result<String> {
        let tag = kind.tag();
        let index = self.index_hash(&tag, &kind.normalise(value));

        if let Some(seq) = self
            .connection
            .prepare_cached("SELECT seq FROM mappings WHERE index_hash = ?1")
            .context("preparing the mapping lookup")?
            .query_row(params![index.as_slice()], |row| row.get::<_, i64>(0))
            .optional()
            .context("looking up an existing mapping")?
        {
            return Ok(placeholder(&tag, seq));
        }

        let transaction = self
            .connection
            .transaction()
            .context("starting a vault transaction")?;
        let seq: i64 = transaction
            .query_row(
                "SELECT COALESCE(MAX(seq), 0) + 1 FROM mappings WHERE tag = ?1",
                params![tag],
                |row| row.get(0),
            )
            .context("allocating the next placeholder number")?;
        // Sealed only once the sequence is known, so the ciphertext is bound to
        // the exact (tag, seq) it will be read back under. Borrowing the cipher
        // field directly keeps it disjoint from the transaction's borrow of the
        // connection field.
        let (nonce, ciphertext) = seal(&self.cipher, &tag, seq, value)?;
        transaction
            .execute(
                "INSERT INTO mappings (tag, seq, index_hash, nonce, ciphertext, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
                params![tag, seq, index.as_slice(), nonce.as_slice(), ciphertext],
            )
            .context("storing a new mapping")?;
        transaction
            .commit()
            .context("committing a vault transaction")?;

        Ok(placeholder(&tag, seq))
    }

    /// Returns the value behind a placeholder, or `None` if this vault has
    /// never issued it.
    ///
    /// # Errors
    ///
    /// Returns an error if the database cannot be read, or if the stored value
    /// fails to decrypt because the key does not match this vault.
    pub fn value_for(&self, tag: &str, seq: i64) -> Result<Option<String>> {
        let row = self
            .connection
            .prepare_cached("SELECT nonce, ciphertext FROM mappings WHERE tag = ?1 AND seq = ?2")
            .context("preparing the placeholder lookup")?
            .query_row(params![tag, seq], |row| {
                Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .optional()
            .context("looking up a placeholder")?;

        match row {
            Some((nonce, ciphertext)) => self.open_sealed(tag, seq, &nonce, &ciphertext).map(Some),
            None => Ok(None),
        }
    }

    /// Lists stored mappings without revealing any value.
    ///
    /// # Errors
    ///
    /// Returns an error if the database cannot be read.
    pub fn entries(&self) -> Result<Vec<Entry>> {
        let mut statement = self
            .connection
            .prepare("SELECT tag, seq, created_at FROM mappings ORDER BY tag, seq")
            .context("preparing the mapping listing")?;
        let rows = statement
            .query_map([], |row| {
                let tag: String = row.get(0)?;
                let seq: i64 = row.get(1)?;
                let created_at: String = row.get(2)?;
                Ok(Entry {
                    tag,
                    seq,
                    created_at,
                })
            })
            .context("listing mappings")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("reading mappings")
    }

    /// Deletes every mapping, making prior sanitised output unrecoverable.
    ///
    /// # Errors
    ///
    /// Returns an error if the database cannot be written to.
    pub fn purge(&self) -> Result<usize> {
        let removed = self
            .connection
            .execute("DELETE FROM mappings", [])
            .context("purging the vault")?;
        self.connection
            .execute_batch("VACUUM")
            .context("compacting the vault after purge")?;
        Ok(removed)
    }

    /// The lookup key for a value: keyed so that the database cannot be
    /// probed for a guessed value without the key file.
    fn index_hash(&self, tag: &str, normalised: &str) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new_keyed(&self.index_key);
        hasher.update(tag.as_bytes());
        hasher.update(b"\0");
        hasher.update(normalised.as_bytes());
        *hasher.finalize().as_bytes()
    }

    fn open_sealed(&self, tag: &str, seq: i64, nonce: &[u8], ciphertext: &[u8]) -> Result<String> {
        let nonce: [u8; NONCE_LEN] = nonce
            .try_into()
            .map_err(|_| anyhow!("vault row has a malformed nonce; the database may be corrupt"))?;
        let plaintext = self
            .cipher
            .decrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: ciphertext,
                    aad: associated_data(tag, seq).as_bytes(),
                },
            )
            .map_err(|_| {
                anyhow!(
                    "decrypting a vault value failed; the key file does not match this vault, or the database was tampered with"
                )
            })?;
        String::from_utf8(plaintext).context("a vault value is not valid UTF-8")
    }
}

/// Encrypts `value`, binding the ciphertext to its `(tag, seq)` so a row
/// cannot be silently moved to another entity kind or swapped with another
/// sequence under the same tag.
///
/// A free function rather than a method so the caller can hold a transaction on
/// the connection field while borrowing only the cipher field here.
fn seal(
    cipher: &Aes256Gcm,
    tag: &str,
    seq: i64,
    value: &str,
) -> Result<([u8; NONCE_LEN], Vec<u8>)> {
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::rng().fill_bytes(&mut nonce_bytes);
    let ciphertext = cipher
        .encrypt(
            &Nonce::from(nonce_bytes),
            Payload {
                msg: value.as_bytes(),
                aad: associated_data(tag, seq).as_bytes(),
            },
        )
        .map_err(|_| anyhow!("encrypting a vault value failed"))?;
    Ok((nonce_bytes, ciphertext))
}

/// The associated data a value is sealed under: its tag and sequence, so
/// neither can be changed without the decryption failing.
fn associated_data(tag: &str, seq: i64) -> String {
    format!("{tag}\0{seq}")
}

/// Formats a placeholder. Double brackets survive markdown rendering and
/// model round-trips without being reinterpreted.
#[must_use]
pub fn placeholder(tag: &str, seq: i64) -> String {
    format!("[[{tag}_{seq}]]")
}

fn initialise_schema(connection: &Connection) -> Result<()> {
    connection
        .execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS mappings (
                 id         INTEGER PRIMARY KEY,
                 tag        TEXT    NOT NULL,
                 seq        INTEGER NOT NULL,
                 index_hash BLOB    NOT NULL,
                 nonce      BLOB    NOT NULL,
                 ciphertext BLOB    NOT NULL,
                 created_at TEXT    NOT NULL,
                 UNIQUE (index_hash),
                 UNIQUE (tag, seq)
             );",
        )
        .context("initialising the vault schema")
}

/// Reads the master key, generating one on first use.
fn load_or_create_key(path: &Path) -> Result<Zeroizing<[u8; 32]>> {
    if let Some(parent) = path.parent() {
        create_private_dir(parent)?;
    }

    if path.exists() {
        let bytes = std::fs::read(path)
            .with_context(|| format!("reading the vault key at {}", path.display()))?;
        let bytes: [u8; 32] = bytes.as_slice().try_into().map_err(|_| {
            anyhow!(
                "the vault key at {} is {} bytes; expected 32",
                path.display(),
                bytes.len()
            )
        })?;
        return Ok(Zeroizing::new(bytes));
    }

    let mut key = Zeroizing::new([0u8; 32]);
    rand::rng().fill_bytes(&mut *key);
    write_private_file(path, &*key)
        .with_context(|| format!("writing a new vault key to {}", path.display()))?;
    Ok(key)
}

/// Writes a file that is owner-only from the moment it exists.
///
/// Writing first and restricting afterwards would leave the key readable by
/// anyone for the duration, since the default umask usually grants group and
/// world read.
fn write_private_file(path: &Path, contents: &[u8]) -> Result<()> {
    use std::io::Write as _;

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(contents)?;
    file.sync_all()?;
    Ok(())
}

/// Creates a directory readable only by its owner.
///
/// A directory that already exists is left exactly as it is. It belongs to
/// whoever made it, and tightening it is not this tool's decision to take:
/// inside a container the vault's parent is a mounted volume owned by another
/// user, where the attempt fails outright and takes the whole command with
/// it.
///
/// Nothing is lost by that. The key file is created `0600` in one step by
/// [`write_private_file`], and the database likewise, so the protection that
/// matters does not depend on the directory's mode.
fn create_private_dir(path: &Path) -> Result<()> {
    if path.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(path)
        .with_context(|| format!("creating directory {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("restricting permissions on {}", path.display()))?;
    }
    Ok(())
}

/// Restricts a file to owner read and write.
fn restrict_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("restricting permissions on {}", path.display()))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Tightens the WAL sidecars to owner-only.
///
/// A sidecar only exists once WAL has work to persist, so one that is absent is
/// expected rather than a failure.
fn restrict_sidecars(db_path: &Path) -> Result<()> {
    for suffix in ["-wal", "-shm"] {
        let mut name = db_path.as_os_str().to_owned();
        name.push(suffix);
        let sidecar = PathBuf::from(name);
        if sidecar.exists() {
            restrict_permissions(&sidecar)?;
        }
    }
    Ok(())
}

/// The default vault location, `~/.oboro/vault.db`.
///
/// # Errors
///
/// Returns an error if the home directory cannot be determined.
pub fn default_db_path() -> Result<PathBuf> {
    Ok(oboro_home()?.join("vault.db"))
}

/// The default key location, `~/.oboro/key`.
///
/// # Errors
///
/// Returns an error if the home directory cannot be determined.
pub fn default_key_path() -> Result<PathBuf> {
    Ok(oboro_home()?.join("key"))
}

fn oboro_home() -> Result<PathBuf> {
    let home = dirs::home_dir()
        .ok_or_else(|| anyhow!("cannot determine the home directory; pass --vault explicitly"))?;
    Ok(home.join(".oboro"))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestVault {
        vault: Vault,
        dir: tempfile::TempDir,
    }

    impl TestVault {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("temporary directory");
            let vault = Vault::open(&dir.path().join("vault.db"), &dir.path().join("key"))
                .expect("opening a fresh vault");
            Self { vault, dir }
        }

        fn path(&self) -> &Path {
            self.dir.path()
        }

        fn db(&self) -> PathBuf {
            self.dir.path().join("vault.db")
        }

        fn key(&self) -> PathBuf {
            self.dir.path().join("key")
        }
    }

    #[test]
    fn the_same_value_always_yields_the_same_placeholder() {
        let mut vault = TestVault::new().vault;
        let first = vault
            .placeholder_for(&EntityKind::Person, "Jean Dupont")
            .expect("allocating");
        let second = vault
            .placeholder_for(&EntityKind::Person, "Jean Dupont")
            .expect("allocating again");
        assert_eq!(first, second);
        assert_eq!(first, "[[PERSON_1]]");
    }

    #[test]
    fn different_values_yield_increasing_placeholders() {
        let mut vault = TestVault::new().vault;
        assert_eq!(
            vault
                .placeholder_for(&EntityKind::Person, "Jean")
                .expect("allocating"),
            "[[PERSON_1]]"
        );
        assert_eq!(
            vault
                .placeholder_for(&EntityKind::Person, "Marie")
                .expect("allocating"),
            "[[PERSON_2]]"
        );
    }

    #[test]
    fn numbering_is_independent_per_kind() {
        let mut vault = TestVault::new().vault;
        assert_eq!(
            vault
                .placeholder_for(&EntityKind::Person, "Jean")
                .expect("allocating"),
            "[[PERSON_1]]"
        );
        assert_eq!(
            vault
                .placeholder_for(&EntityKind::Email, "jean@example.com")
                .expect("allocating"),
            "[[EMAIL_1]]"
        );
    }

    #[test]
    fn formatting_differences_reuse_one_placeholder() {
        let mut vault = TestVault::new().vault;
        let spaced = vault
            .placeholder_for(&EntityKind::Phone, "06 12 34 56 78")
            .expect("allocating");
        let compact = vault
            .placeholder_for(&EntityKind::Phone, "0612345678")
            .expect("allocating");
        assert_eq!(spaced, compact);

        let iban_spaced = vault
            .placeholder_for(&EntityKind::Iban, "FR14 2004 1010")
            .expect("allocating");
        let iban_compact = vault
            .placeholder_for(&EntityKind::Iban, "fr1420041010")
            .expect("allocating");
        assert_eq!(iban_spaced, iban_compact);
    }

    #[test]
    fn values_survive_the_encryption_round_trip() {
        let mut vault = TestVault::new().vault;
        let value = "Jean Dupont, 12 rue de la Paix";
        vault
            .placeholder_for(&EntityKind::Person, value)
            .expect("allocating");
        let restored = vault.value_for("PERSON", 1).expect("reading back");
        assert_eq!(restored.as_deref(), Some(value));
    }

    #[test]
    fn the_first_stored_spelling_is_the_one_restored() {
        let mut vault = TestVault::new().vault;
        vault
            .placeholder_for(&EntityKind::Person, "Jean Dupont")
            .expect("allocating");
        vault
            .placeholder_for(&EntityKind::Person, "JEAN DUPONT")
            .expect("allocating");
        assert_eq!(
            vault
                .value_for("PERSON", 1)
                .expect("reading back")
                .as_deref(),
            Some("Jean Dupont")
        );
    }

    #[test]
    fn unknown_placeholders_resolve_to_nothing() {
        let vault = TestVault::new().vault;
        assert!(
            vault
                .value_for("PERSON", 42)
                .expect("reading back")
                .is_none()
        );
    }

    #[test]
    fn mappings_persist_across_reopening() {
        let mut fixture = TestVault::new();
        fixture
            .vault
            .placeholder_for(&EntityKind::Person, "Jean Dupont")
            .expect("allocating");
        let reopened = Vault::open(&fixture.db(), &fixture.key()).expect("reopening");
        assert_eq!(
            reopened
                .value_for("PERSON", 1)
                .expect("reading back")
                .as_deref(),
            Some("Jean Dupont")
        );
    }

    #[test]
    fn a_foreign_key_cannot_decrypt_the_vault() {
        let mut fixture = TestVault::new();
        fixture
            .vault
            .placeholder_for(&EntityKind::Person, "Jean Dupont")
            .expect("allocating");

        let other_key = fixture.path().join("other-key");
        std::fs::write(&other_key, [7u8; 32]).expect("writing a foreign key");
        let intruder = Vault::open(&fixture.db(), &other_key).expect("opening with a foreign key");
        assert!(
            intruder.value_for("PERSON", 1).is_err(),
            "a foreign key must not decrypt stored values"
        );
    }

    #[test]
    fn listing_reports_placeholders_without_values() {
        let mut vault = TestVault::new().vault;
        vault
            .placeholder_for(&EntityKind::Person, "Jean Dupont")
            .expect("allocating");
        let entries = vault.entries().expect("listing");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].placeholder(), "[[PERSON_1]]");
        assert_eq!(entries[0].tag, "PERSON");
        assert_eq!(entries[0].seq, 1);
    }

    #[test]
    fn purge_removes_every_mapping() {
        let mut vault = TestVault::new().vault;
        vault
            .placeholder_for(&EntityKind::Person, "Jean Dupont")
            .expect("allocating");
        assert_eq!(vault.purge().expect("purging"), 1);
        assert!(vault.entries().expect("listing").is_empty());
        assert!(
            vault
                .value_for("PERSON", 1)
                .expect("reading back")
                .is_none()
        );
    }

    #[test]
    fn swapping_two_rows_under_one_tag_is_detected() {
        let fixture = TestVault::new();
        let mut vault = fixture.vault;
        vault
            .placeholder_for(&EntityKind::Person, "Jean Dupont")
            .expect("allocating");
        vault
            .placeholder_for(&EntityKind::Person, "Marie Curie")
            .expect("allocating");

        // Swap the sealed payloads of seq 1 and seq 2. Binding the sequence
        // into the associated data means each now decrypts under the wrong
        // seq, so both must fail rather than silently returning the other name.
        vault
            .connection
            .execute_batch(
                "UPDATE mappings SET seq = 0 WHERE tag = 'PERSON' AND seq = 1;
                 UPDATE mappings SET seq = 1 WHERE tag = 'PERSON' AND seq = 2;
                 UPDATE mappings SET seq = 2 WHERE tag = 'PERSON' AND seq = 0;",
            )
            .expect("swapping rows");

        assert!(
            vault.value_for("PERSON", 1).is_err(),
            "a row moved to another sequence must not decrypt"
        );
        assert!(vault.value_for("PERSON", 2).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn the_wal_sidecars_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = TestVault::new();
        let mut vault = fixture.vault;
        vault
            .placeholder_for(&EntityKind::Person, "Jean Dupont")
            .expect("allocating");

        let wal = {
            let mut name = fixture.dir.path().join("vault.db").into_os_string();
            name.push("-wal");
            PathBuf::from(name)
        };
        assert!(wal.exists(), "WAL mode must create the sidecar");
        assert_eq!(
            std::fs::metadata(&wal)
                .expect("reading")
                .permissions()
                .mode()
                & 0o777,
            0o600,
            "the WAL sidecar must be owner-only, not left to the umask"
        );
    }

    #[test]
    fn a_malformed_key_file_is_rejected() {
        let fixture = TestVault::new();
        let key = fixture.path().join("short-key");
        std::fs::write(&key, b"too short").expect("writing a short key");
        let error = Vault::open(&fixture.path().join("other.db"), &key)
            .err()
            .expect("a short key must be rejected");
        assert!(format!("{error:#}").contains("expected 32"));
    }

    /// A pre-existing directory is somebody else's, and in a container it is
    /// a mounted volume this process cannot chmod. Failing there would make
    /// the tool unusable in exactly the place it is most wanted.
    #[cfg(unix)]
    #[test]
    fn a_vault_opens_in_a_directory_it_did_not_create() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("temporary directory");
        let store = dir.path().join("mounted");
        std::fs::create_dir(&store).expect("creating the directory first");
        std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o755))
            .expect("loosening it, as a volume mount would be");

        let mut vault = Vault::open(&store.join("vault.db"), &store.join("key"))
            .expect("a directory we did not create must not stop the vault opening");
        vault
            .placeholder_for(&EntityKind::Person, "Jean Dupont")
            .expect("allocating");

        assert_eq!(
            std::fs::metadata(&store)
                .expect("reading")
                .permissions()
                .mode()
                & 0o777,
            0o755,
            "an existing directory must be left exactly as it was"
        );
        assert_eq!(
            std::fs::metadata(store.join("key"))
                .expect("reading")
                .permissions()
                .mode()
                & 0o777,
            0o600,
            "the key must still be owner-only regardless"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_key_file_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = TestVault::new();
        let mode = std::fs::metadata(fixture.key())
            .expect("reading key metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "key file must be owner-only");
    }
}
