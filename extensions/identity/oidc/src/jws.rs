//! Compact signed tokens and the published keys that verify them.
//!
//! An ID token is a JWS in its compact form: a header that names the algorithm and the key, the
//! claims, and the signature over both, each in base64url and joined by dots. The keys are the
//! provider's JWK Set. Only what a provider signs with and anybody can check is taken: RSA with
//! PKCS #1 v1.5 or PSS padding over SHA-2 (moduli of 2048 to 8192 bits), ECDSA on P-256 and P-384,
//! and Ed25519. A token that is not signed (`none`), one signed with a shared secret (`HS*`), one that
//! is encrypted (five parts), one whose header names an extension this reader does not know (`crit`)
//! and an access token (`typ` `at+jwt`) are refused. A published key that is for encryption, or of a
//! kind this reader does not know, is passed over and not counted.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use orch_identity_contract::provider::ProviderError;
use orch_types::Json;
use ring::error::Unspecified;
use ring::signature::{
    ECDSA_P256_SHA256_FIXED, ECDSA_P384_SHA384_FIXED, ED25519, RSA_PKCS1_2048_8192_SHA256,
    RSA_PKCS1_2048_8192_SHA384, RSA_PKCS1_2048_8192_SHA512, RSA_PSS_2048_8192_SHA256,
    RSA_PSS_2048_8192_SHA384, RSA_PSS_2048_8192_SHA512, RsaParameters, RsaPublicKeyComponents,
    UnparsedPublicKey,
};
use serde_json::Map;

/// The fewest bytes of an RSA modulus that is taken: 2048 bits, below which a key is no longer
/// believed to resist factoring, and the least the signature library checks.
const RSA_MIN_MODULUS_BYTES: usize = 256;
/// The most bytes of an RSA modulus that is taken: 8192 bits, the most the signature library checks.
const RSA_MAX_MODULUS_BYTES: usize = 1024;
/// The bytes of one coordinate of a point on P-256.
const P256_COORDINATE_BYTES: usize = 32;
/// The bytes of one coordinate of a point on P-384.
const P384_COORDINATE_BYTES: usize = 48;
/// The bytes of an Ed25519 public key.
const ED25519_KEY_BYTES: usize = 32;
/// The first byte of a point written whole (uncompressed), the form the library reads an ECDSA key in.
const UNCOMPRESSED_POINT: u8 = 0x04;
/// The values of `typ` that mark an access token, which is never taken for an identity.
const ACCESS_TOKEN_TYPES: [&str; 2] = ["at+jwt", "application/at+jwt"];

/// An algorithm an ID token may be signed with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Algorithm {
    /// RSA with PKCS #1 v1.5 padding and SHA-256: the one every provider of OpenID Connect offers.
    Rs256,
    /// RSA with PKCS #1 v1.5 padding and SHA-384.
    Rs384,
    /// RSA with PKCS #1 v1.5 padding and SHA-512.
    Rs512,
    /// RSA with PSS padding and SHA-256.
    Ps256,
    /// RSA with PSS padding and SHA-384.
    Ps384,
    /// RSA with PSS padding and SHA-512.
    Ps512,
    /// ECDSA on P-256 with SHA-256.
    Es256,
    /// ECDSA on P-384 with SHA-384.
    Es384,
    /// Ed25519.
    EdDsa,
}

impl Algorithm {
    /// Every algorithm that is taken.
    pub const ALL: [Self; 9] = [
        Self::Rs256,
        Self::Rs384,
        Self::Rs512,
        Self::Ps256,
        Self::Ps384,
        Self::Ps512,
        Self::Es256,
        Self::Es384,
        Self::EdDsa,
    ];

    /// The algorithm a header or a key names, if it is one that is taken. Ed25519 is named `EdDSA`, or
    /// `Ed25519` by the providers that name the curve with it.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "RS256" => Some(Self::Rs256),
            "RS384" => Some(Self::Rs384),
            "RS512" => Some(Self::Rs512),
            "PS256" => Some(Self::Ps256),
            "PS384" => Some(Self::Ps384),
            "PS512" => Some(Self::Ps512),
            "ES256" => Some(Self::Es256),
            "ES384" => Some(Self::Es384),
            "EdDSA" | "Ed25519" => Some(Self::EdDsa),
            _ => None,
        }
    }

    /// Its name as a header writes it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Rs256 => "RS256",
            Self::Rs384 => "RS384",
            Self::Rs512 => "RS512",
            Self::Ps256 => "PS256",
            Self::Ps384 => "PS384",
            Self::Ps512 => "PS512",
            Self::Es256 => "ES256",
            Self::Es384 => "ES384",
            Self::EdDsa => "EdDSA",
        }
    }

    /// How an RSA signature of this algorithm is checked; none for the others.
    fn rsa_parameters(self) -> Option<&'static RsaParameters> {
        match self {
            Self::Rs256 => Some(&RSA_PKCS1_2048_8192_SHA256),
            Self::Rs384 => Some(&RSA_PKCS1_2048_8192_SHA384),
            Self::Rs512 => Some(&RSA_PKCS1_2048_8192_SHA512),
            Self::Ps256 => Some(&RSA_PSS_2048_8192_SHA256),
            Self::Ps384 => Some(&RSA_PSS_2048_8192_SHA384),
            Self::Ps512 => Some(&RSA_PSS_2048_8192_SHA512),
            Self::Es256 | Self::Es384 | Self::EdDsa => None,
        }
    }
}

/// What a key checks with.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Material {
    /// An RSA public key: the modulus and the exponent, big-endian, without leading zeros.
    Rsa { n: Vec<u8>, e: Vec<u8> },
    /// A point on P-256, written whole.
    P256(Vec<u8>),
    /// A point on P-384, written whole.
    P384(Vec<u8>),
    /// An Ed25519 public key.
    Ed25519(Vec<u8>),
}

impl Material {
    /// Whether a signature made with `alg` can be checked with this.
    fn fits(&self, alg: Algorithm) -> bool {
        match self {
            Self::Rsa { .. } => alg.rsa_parameters().is_some(),
            Self::P256(_) => alg == Algorithm::Es256,
            Self::P384(_) => alg == Algorithm::Es384,
            Self::Ed25519(_) => alg == Algorithm::EdDsa,
        }
    }
}

/// One published key that can check a signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Jwk {
    /// What the provider calls it (`kid`), where it names it.
    pub kid: Option<String>,
    /// The one algorithm it is for (`alg`), where the provider says.
    pub alg: Option<Algorithm>,
    material: Material,
}

impl Jwk {
    /// Reads one member of a key set; `None` for a key that is not for signatures (`use` other than
    /// `sig`, or `key_ops` without `verify`), that is for an algorithm that is not taken, or that is of
    /// a kind or a size this reader does not take.
    #[must_use]
    pub fn read(value: &Json) -> Option<Self> {
        let text = |name: &str| value.get(name).and_then(Json::as_str);
        if text("use").is_some_and(|usage| usage != "sig") {
            return None;
        }
        if let Some(operations) = value.get("key_ops") {
            let verifies = operations.as_array().is_some_and(|operations| {
                operations
                    .iter()
                    .any(|operation| operation.as_str() == Some("verify"))
            });
            if !verifies {
                return None;
            }
        }
        let alg = match text("alg") {
            Some(name) => Some(Algorithm::from_name(name)?),
            None => None,
        };
        let bytes =
            |name: &str| text(name).and_then(|encoded| URL_SAFE_NO_PAD.decode(encoded).ok());
        let material = match (text("kty")?, text("crv")) {
            ("RSA", _) => {
                let n = without_leading_zeros(bytes("n")?);
                let e = without_leading_zeros(bytes("e")?);
                if !(RSA_MIN_MODULUS_BYTES..=RSA_MAX_MODULUS_BYTES).contains(&n.len())
                    || e.is_empty()
                {
                    return None;
                }
                Material::Rsa { n, e }
            }
            ("EC", Some("P-256")) => {
                Material::P256(point(&bytes("x")?, &bytes("y")?, P256_COORDINATE_BYTES)?)
            }
            ("EC", Some("P-384")) => {
                Material::P384(point(&bytes("x")?, &bytes("y")?, P384_COORDINATE_BYTES)?)
            }
            ("OKP", Some("Ed25519")) => {
                let x = bytes("x")?;
                if x.len() != ED25519_KEY_BYTES {
                    return None;
                }
                Material::Ed25519(x)
            }
            _ => return None,
        };
        if alg.is_some_and(|alg| !material.fits(alg)) {
            return None;
        }
        Some(Self {
            kid: text("kid").map(str::to_owned),
            alg,
            material,
        })
    }

    /// Whether it can check a signature made with `alg`: a key of that kind, and for that algorithm
    /// where the key names one.
    #[must_use]
    pub fn fits(&self, alg: Algorithm) -> bool {
        self.alg.is_none_or(|own| own == alg) && self.material.fits(alg)
    }

    /// Checks that `signature` is this key's over `message`, made with `alg`.
    pub fn verify(
        &self,
        alg: Algorithm,
        message: &[u8],
        signature: &[u8],
    ) -> Result<(), ProviderError> {
        let checked = if self.fits(alg) {
            match &self.material {
                Material::Rsa { n, e } => match alg.rsa_parameters() {
                    Some(parameters) => RsaPublicKeyComponents {
                        n: n.as_slice(),
                        e: e.as_slice(),
                    }
                    .verify(parameters, message, signature),
                    None => Err(Unspecified),
                },
                Material::P256(point) => UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, point)
                    .verify(message, signature),
                Material::P384(point) => UnparsedPublicKey::new(&ECDSA_P384_SHA384_FIXED, point)
                    .verify(message, signature),
                Material::Ed25519(x) => {
                    UnparsedPublicKey::new(&ED25519, x).verify(message, signature)
                }
            }
        } else {
            Err(Unspecified)
        };
        checked.map_err(|Unspecified| {
            ProviderError::new("the signature of the ID token is not one of the key it names")
        })
    }
}

/// The bytes of a number without the zeros it was padded with in front.
fn without_leading_zeros(bytes: Vec<u8>) -> Vec<u8> {
    match bytes.iter().position(|byte| *byte != 0) {
        Some(0) => bytes,
        Some(start) => bytes.get(start..).map(<[u8]>::to_vec).unwrap_or_default(),
        None => Vec::new(),
    }
}

/// A point on a curve written whole from its two coordinates of `size` bytes each.
fn point(x: &[u8], y: &[u8], size: usize) -> Option<Vec<u8>> {
    if x.len() != size || y.len() != size {
        return None;
    }
    let mut whole = Vec::with_capacity(1 + 2 * size);
    whole.push(UNCOMPRESSED_POINT);
    whole.extend_from_slice(x);
    whole.extend_from_slice(y);
    Some(whole)
}

/// The keys an issuer publishes (its JWK Set), those of them this reader can use.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeySet {
    /// The keys that can check a signature.
    pub keys: Vec<Jwk>,
}

impl KeySet {
    /// Reads a published key set: a document with a list of keys, of which those this reader cannot
    /// use are passed over.
    pub fn read(document: &Json) -> Result<Self, ProviderError> {
        let keys = document
            .get("keys")
            .and_then(Json::as_array)
            .ok_or_else(|| ProviderError::new("the key set holds no list of keys"))?;
        Ok(Self {
            keys: keys.iter().filter_map(Jwk::read).collect(),
        })
    }

    /// The key a token signed with `alg` names: the one called `kid` that fits the algorithm, or, for
    /// a token that names none, the one key that fits it, where only one does.
    #[must_use]
    pub fn find(&self, kid: Option<&str>, alg: Algorithm) -> Option<&Jwk> {
        match kid {
            Some(kid) => self
                .keys
                .iter()
                .find(|key| key.kid.as_deref() == Some(kid) && key.fits(alg)),
            None => {
                let mut fitting = self.keys.iter().filter(|key| key.fits(alg));
                let first = fitting.next()?;
                fitting.next().is_none().then_some(first)
            }
        }
    }
}

/// A signed token, read and not yet believed.
#[derive(Debug, Clone)]
pub struct Signed {
    /// The algorithm its header names.
    pub alg: Algorithm,
    /// The key its header names, where it names one.
    pub kid: Option<String>,
    /// What it says.
    pub claims: Map<String, Json>,
    /// What was signed: the header and the claims as they came, joined by a dot.
    signing_input: String,
    /// The signature.
    signature: Vec<u8>,
}

impl Signed {
    /// Reads a token in the compact form, and refuses what is never taken (see the module).
    pub fn read(token: &str) -> Result<Self, ProviderError> {
        let mut parts = token.split('.');
        let (Some(header_text), Some(claims_text), Some(signature_text), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(ProviderError::new(
                "the ID token is not a signed token of three parts",
            ));
        };
        let header = decode_json(header_text, "header")?;
        let named = header
            .get("alg")
            .and_then(Json::as_str)
            .ok_or_else(|| ProviderError::new("the ID token names no algorithm"))?;
        let alg = Algorithm::from_name(named).ok_or_else(|| {
            ProviderError::new(format!(
                "the ID token is signed with {named}, which is not taken"
            ))
        })?;
        if header.get("crit").is_some() {
            return Err(ProviderError::new(
                "the ID token names a header extension that is not known here",
            ));
        }
        if header
            .get("typ")
            .and_then(Json::as_str)
            .is_some_and(|kind| {
                ACCESS_TOKEN_TYPES
                    .iter()
                    .any(|access| kind.eq_ignore_ascii_case(access))
            })
        {
            return Err(ProviderError::new(
                "an access token was given where an ID token belongs",
            ));
        }
        let kid = header.get("kid").and_then(Json::as_str).map(str::to_owned);
        let Json::Object(claims) = decode_json(claims_text, "claims")? else {
            return Err(ProviderError::new(
                "the claims of the ID token are not an object",
            ));
        };
        let signature = URL_SAFE_NO_PAD
            .decode(signature_text)
            .map_err(|_| ProviderError::new("the signature of the ID token is not base64url"))?;
        Ok(Self {
            alg,
            kid,
            claims,
            signing_input: format!("{header_text}.{claims_text}"),
            signature,
        })
    }

    /// Checks the signature with `key`.
    pub fn verify(&self, key: &Jwk) -> Result<(), ProviderError> {
        key.verify(self.alg, self.signing_input.as_bytes(), &self.signature)
    }
}

/// One part of a token, decoded from base64url and read as JSON; `what` names it in the error.
fn decode_json(part: &str, what: &str) -> Result<Json, ProviderError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(part)
        .map_err(|_| ProviderError::new(format!("the {what} of the ID token is not base64url")))?;
    serde_json::from_slice(&bytes)
        .map_err(|_| ProviderError::new(format!("the {what} of the ID token is not JSON")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_testkit::oidc::{Alg, TestKey};
    use serde_json::json;

    fn claims() -> Json {
        json!({ "iss": "https://login.acme.example", "sub": "248289761001", "aud": "orch",
            "iat": 1_791_000_000, "exp": 1_791_000_300, "nonce": "n-1" })
    }

    fn encoded(value: &Json) -> String {
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(value).unwrap())
    }

    #[test]
    fn a_token_signed_by_a_published_key_verifies_with_every_algorithm_that_is_taken() {
        for alg in [Alg::Rs256, Alg::Ps256, Alg::Es256, Alg::Es384, Alg::EdDsa] {
            let key = TestKey::new(alg, "k1");
            let keys = KeySet::read(&json!({ "keys": [key.jwk()] })).unwrap();
            assert_eq!(keys.keys.len(), 1, "{alg:?}");
            let token = Signed::read(&key.sign(&claims())).unwrap();
            assert_eq!(token.alg.name(), alg.name());
            assert_eq!(token.kid.as_deref(), Some("k1"));
            let found = keys.find(Some("k1"), token.alg).unwrap();
            token.verify(found).unwrap();
            assert_eq!(token.claims["sub"], "248289761001");
        }
    }

    #[test]
    fn a_changed_claim_or_another_key_does_not_verify() {
        let key = TestKey::numbered(Alg::Rs256, "k1", 0);
        let other = TestKey::new(Alg::Es256, "k2");
        let signed = key.sign(&claims());
        let mut parts: Vec<&str> = signed.split('.').collect();
        let mut changed = claims();
        changed["sub"] = json!("somebody-else");
        let changed = encoded(&changed);
        parts[1] = &changed;
        let forged = Signed::read(&parts.join(".")).unwrap();
        let jwk = Jwk::read(&key.jwk()).unwrap();
        assert!(
            forged.verify(&jwk).is_err(),
            "a claim changed after signing"
        );
        let genuine = Signed::read(&signed).unwrap();
        let elsewhere = Jwk::read(&other.jwk()).unwrap();
        assert!(
            genuine.verify(&elsewhere).is_err(),
            "a key of another kind checks nothing"
        );
        let same_kind = Jwk::read(&TestKey::numbered(Alg::Rs256, "k3", 1).jwk()).unwrap();
        assert!(genuine.verify(&same_kind).is_err(), "another RSA key");
    }

    #[test]
    fn an_unsigned_token_one_signed_with_a_shared_secret_or_an_encrypted_one_is_refused() {
        let body = encoded(&claims());
        for header in [
            json!({ "alg": "none" }),
            json!({ "alg": "HS256" }),
            json!({ "alg": "RS256", "crit": ["exp"] }),
            json!({ "alg": "RS256", "typ": "at+jwt" }),
            json!({ "typ": "JWT" }),
        ] {
            let token = format!("{}.{body}.c2lnbmF0dXJl", encoded(&header));
            assert!(Signed::read(&token).is_err(), "{header}");
        }
        assert!(
            Signed::read("a.b.c.d.e").is_err(),
            "five parts are encrypted"
        );
        assert!(Signed::read(&format!("{}.{body}", encoded(&json!({ "alg": "RS256" })))).is_err());
        let fine = format!(
            "{}.{body}.c2lnbmF0dXJl",
            encoded(&json!({ "alg": "RS256", "typ": "JWT" }))
        );
        assert!(Signed::read(&fine).is_ok());
    }

    #[test]
    fn keys_for_encryption_unknown_kinds_and_small_moduli_are_passed_over() {
        let rsa = TestKey::new(Alg::Rs256, "sig").jwk();
        let mut encryption = rsa.clone();
        encryption["use"] = json!("enc");
        let mut wrapping = rsa.clone();
        wrapping["key_ops"] = json!(["wrapKey"]);
        let mut wrong_alg = rsa.clone();
        wrong_alg["alg"] = json!("ES256");
        let mut small = rsa.clone();
        small["n"] = json!(URL_SAFE_NO_PAD.encode([0xc5_u8; 128]));
        let secret = json!({ "kty": "oct", "k": "c2VjcmV0", "kid": "hmac" });
        let set =
            KeySet::read(&json!({ "keys": [rsa, encryption, wrapping, wrong_alg, small, secret] }))
                .unwrap();
        assert_eq!(set.keys.len(), 1);
        assert!(KeySet::read(&json!({ "nothing": [] })).is_err());
    }

    #[test]
    fn a_token_that_names_no_key_finds_the_one_key_that_fits_and_no_other() {
        let first = TestKey::new(Alg::Es256, "a");
        let second = TestKey::new(Alg::Es256, "b");
        let mut nameless = first.jwk();
        nameless.as_object_mut().unwrap().remove("kid");
        let one = KeySet::read(&json!({ "keys": [nameless] })).unwrap();
        assert!(one.find(None, Algorithm::Es256).is_some());
        assert!(one.find(None, Algorithm::Rs256).is_none());
        let two = KeySet::read(&json!({ "keys": [first.jwk(), second.jwk()] })).unwrap();
        assert!(
            two.find(None, Algorithm::Es256).is_none(),
            "which of two is not guessed"
        );
        assert_eq!(
            two.find(Some("b"), Algorithm::Es256)
                .unwrap()
                .kid
                .as_deref(),
            Some("b")
        );
        assert!(two.find(Some("c"), Algorithm::Es256).is_none());
    }

    #[test]
    fn every_name_is_read_back_as_its_algorithm() {
        for alg in Algorithm::ALL {
            assert_eq!(Algorithm::from_name(alg.name()), Some(alg));
        }
        assert_eq!(Algorithm::from_name("Ed25519"), Some(Algorithm::EdDsa));
        assert_eq!(Algorithm::from_name("none"), None);
        assert_eq!(Algorithm::from_name("HS256"), None);
    }
}
