//! Signing isolation (RS-12). The key is loaded once from an encrypted keystore and is never
//! printed. `Debug` is redacted, and only `wallet()` exposes signing, to the RPC layer.

use std::path::Path;

use alloy::network::EthereumWallet;
use alloy::primitives::Address;
use alloy::signers::local::PrivateKeySigner;

pub struct KeystoreSigner {
    signer: PrivateKeySigner,
}

impl KeystoreSigner {
    /// Decrypts a keystore file. The password comes from the caller, never from the command line.
    pub fn load(keystore: impl AsRef<Path>, password: &str) -> anyhow::Result<Self> {
        let signer = PrivateKeySigner::decrypt_keystore(keystore, password)?;
        Ok(Self { signer })
    }

    pub fn address(&self) -> Address {
        self.signer.address()
    }

    pub fn wallet(&self) -> EthereumWallet {
        EthereumWallet::from(self.signer.clone())
    }
}

impl std::fmt::Debug for KeystoreSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeystoreSigner")
            .field("address", &self.address())
            .field("key", &"<redacted>")
            .finish()
    }
}
