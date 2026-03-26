use rand::rngs::OsRng;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// An X25519 DH key pair for DM key negotiation.
/// The secret scalar is zeroed on drop.
pub struct DhKeyPair {
    secret: StaticSecret,
    public: PublicKey,
}

impl DhKeyPair {
    /// Generate a new random X25519 key pair.
    pub fn generate() -> Self {
        let secret = StaticSecret::random_from_rng(OsRng);
        let public = PublicKey::from(&secret);
        Self { secret, public }
    }

    /// Return a reference to the public key.
    pub fn public_key(&self) -> &PublicKey {
        &self.public
    }

    /// Return the public key as raw bytes.
    pub fn public_key_bytes(&self) -> [u8; 32] {
        self.public.to_bytes()
    }

    /// Perform an X25519 Diffie-Hellman exchange and return the shared secret.
    /// The result is wrapped in [`SharedSecret`] and zeroed on drop.
    pub fn shared_secret(&self, their_public: &PublicKey) -> SharedSecret {
        let raw = self.secret.diffie_hellman(their_public);
        SharedSecret(raw.to_bytes())
    }
}

impl Drop for DhKeyPair {
    fn drop(&mut self) {
        // StaticSecret implements ZeroizeOnDrop internally; this is belt-and-suspenders.
        self.public.zeroize();
    }
}

/// Raw 32-byte output of an X25519 Diffie-Hellman exchange.
/// Bytes are zeroed on drop.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct SharedSecret([u8; 32]);

impl SharedSecret {
    /// Raw shared secret bytes. Zeroize after use.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dh_shared_secret_matches() {
        let alice = DhKeyPair::generate();
        let bob = DhKeyPair::generate();

        let alice_shared = alice.shared_secret(bob.public_key());
        let bob_shared = bob.shared_secret(alice.public_key());

        assert_eq!(alice_shared.as_bytes(), bob_shared.as_bytes());
    }

    #[test]
    fn different_pairs_produce_different_secrets() {
        let alice = DhKeyPair::generate();
        let bob = DhKeyPair::generate();
        let carol = DhKeyPair::generate();

        let ab = alice.shared_secret(bob.public_key());
        let ac = alice.shared_secret(carol.public_key());

        assert_ne!(ab.as_bytes(), ac.as_bytes());
    }

    #[test]
    fn public_key_roundtrip() {
        let pair = DhKeyPair::generate();
        let bytes = pair.public_key_bytes();
        let reconstructed = PublicKey::from(bytes);
        assert_eq!(pair.public_key().as_bytes(), reconstructed.as_bytes());
    }
}
