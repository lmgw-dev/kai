//! The device's key (lmgw's device link design D14, §6.2): a P-256 key generated in software with
//! aws-lc-rs, the only secret of a pairing. Its PKCS#8 lives in Kai's Secret Service item
//! (`crate::secret`), as PEM text; lmgw keeps its public half only, in the device's certificate.
//! Every copy Kai makes of the key's bytes (the DER, its PEM and the PEM's base64) is wiped when
//! dropped ([`Zeroizing`]).

use std::sync::Arc;

use rustls::crypto::CryptoProvider;
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::sign::SigningKey;
use zeroize::Zeroizing;

const PEM_BEGIN: &str = "-----BEGIN PRIVATE KEY-----";
const PEM_END: &str = "-----END PRIVATE KEY-----";

/// The device's private key, PKCS#8 (DER), wiped when dropped. Its `Debug` is redacted and it has
/// no `Display`, so it cannot reach a log line by accident; only the tests compare keys.
#[derive(Clone)]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub struct TlsKey(Zeroizing<Vec<u8>>);

impl std::fmt::Debug for TlsKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TlsKey(<redacted>)")
    }
}

impl TlsKey {
    /// A fresh P-256 key (ECDSA with SHA-256, what lmgw enrols, device link §2.3).
    pub fn generate() -> Result<Self, String> {
        use aws_lc_rs::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair};
        let doc = EcdsaKeyPair::generate_pkcs8(
            &ECDSA_P256_SHA256_ASN1_SIGNING,
            &aws_lc_rs::rand::SystemRandom::new(),
        )
        .map_err(|_| "cannot generate the device's key (aws-lc-rs refused)".to_owned())?;
        Ok(Self(Zeroizing::new(doc.as_ref().to_vec())))
    }

    /// The key as the Secret Service item holds it: PKCS#8 as PEM (`PRIVATE KEY`).
    pub fn to_pem(&self) -> Zeroizing<String> {
        let b64 = Zeroizing::new(lmgw_client::base64::encode(&self.0));
        // Sized up front: the text never reallocates, so no copy of it is left behind.
        let lines = b64.len().div_ceil(64);
        let mut pem = Zeroizing::new(String::with_capacity(
            PEM_BEGIN.len() + PEM_END.len() + b64.len() + lines + 2,
        ));
        pem.push_str(PEM_BEGIN);
        pem.push('\n');
        for line in b64.as_bytes().chunks(64) {
            pem.push_str(std::str::from_utf8(line).expect("base64 is ASCII"));
            pem.push('\n');
        }
        pem.push_str(PEM_END);
        pem.push('\n');
        pem
    }

    /// [`TlsKey::to_pem`]'s text read back. Checked only for its shape: a key that is not a P-256
    /// PKCS#8 is refused where it is used ([`TlsKey::signing_key`]).
    pub fn from_pem(text: &str) -> Result<Self, String> {
        let body = text
            .trim()
            .strip_prefix(PEM_BEGIN)
            .and_then(|r| r.strip_suffix(PEM_END))
            .ok_or("Kai's wallet item does not hold a PEM private key")?;
        let mut b64 = Zeroizing::new(String::with_capacity(body.len()));
        b64.extend(body.split_whitespace());
        lmgw_client::base64::decode(&b64)
            .map(Zeroizing::new)
            .filter(|der| !der.is_empty())
            .map(Self)
            .ok_or_else(|| "Kai's wallet item's private key is not base64".to_owned())
    }

    /// The key as rustls signs with it (`provider`'s key loader).
    pub fn signing_key(&self, provider: &CryptoProvider) -> Result<Arc<dyn SigningKey>, String> {
        // The loader takes its own copy, which rustls's aws-lc-rs provider wipes after parsing.
        provider
            .key_provider
            .load_private_key(PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
                self.0.to_vec(),
            )))
            .map_err(|e| format!("the device's key does not load: {e}"))
    }

    /// The public half as enrolment sends it: the DER SubjectPublicKeyInfo (an EC P-256 key,
    /// uncompressed point, canonical as lmgw requires, device link §2.3).
    pub fn public_key(&self) -> Result<Vec<u8>, String> {
        let signing = self.signing_key(&crate::lmgw_link::tls::provider())?;
        signing
            .public_key()
            .map(|spki| spki.as_ref().to_vec())
            .ok_or_else(|| "the device's key names no public key".to_owned())
    }
}
