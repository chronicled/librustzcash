//! Sapling key components.
//!
//! Implements [section 4.2.2] of the Zcash Protocol Specification.
//!
//! [section 4.2.2]: https://zips.z.cash/protocol/protocol.pdf#saplingkeycomponents

use std::io::{self, Read, Write};

use crate::{
    constants::{PROOF_GENERATION_KEY_GENERATOR, SPENDING_KEY_GENERATOR},
    keys::{prf_expand, OutgoingViewingKey},
    zip32,
};
use ff::PrimeField;
use group::{Group, GroupEncoding};
use subtle::CtOption;

use super::{NullifierDerivingKey, PaymentAddress, ProofGenerationKey, SaplingIvk, ViewingKey};

/// Errors that can occur in the decoding of Sapling spending keys.
pub enum DecodingError {
    /// The length of the byte slice provided for decoding was incorrect.
    LengthInvalid { expected: usize, actual: usize },
    /// Could not decode the `ask` bytes to a jubjub field element.
    InvalidAsk,
    /// Could not decode the `nsk` bytes to a jubjub field element.
    InvalidNsk,
}

/// A Sapling expanded spending key
#[derive(Clone)]
pub struct ExpandedSpendingKey {
    pub ask: jubjub::Fr,
    pub nsk: jubjub::Fr,
    pub ovk: OutgoingViewingKey,
}

/// A Sapling key that provides the capability to view incoming and outgoing transactions.
#[derive(Debug)]
pub struct FullViewingKey {
    pub vk: ViewingKey,
    pub ovk: OutgoingViewingKey,
}

impl ExpandedSpendingKey {
    pub fn from_spending_key(sk: &[u8]) -> Self {
        let ask = jubjub::Fr::from_bytes_wide(prf_expand(sk, &[0x00]).as_array());
        let nsk = jubjub::Fr::from_bytes_wide(prf_expand(sk, &[0x01]).as_array());
        let mut ovk = OutgoingViewingKey([0u8; 32]);
        ovk.0
            .copy_from_slice(&prf_expand(sk, &[0x02]).as_bytes()[..32]);
        ExpandedSpendingKey { ask, nsk, ovk }
    }

    pub fn proof_generation_key(&self) -> ProofGenerationKey {
        ProofGenerationKey {
            ak: SPENDING_KEY_GENERATOR * self.ask,
            nsk: self.nsk,
        }
    }

    /// Decodes the expanded spending key from its serialized representation
    /// as part of the encoding of the extended spending key as defined in
    /// [ZIP 32](https://zips.z.cash/zip-0032)
    pub fn from_bytes(b: &[u8]) -> Result<Self, DecodingError> {
        if b.len() != 96 {
            return Err(DecodingError::LengthInvalid {
                expected: 96,
                actual: b.len(),
            });
        }

        let ask = Option::from(jubjub::Fr::from_repr(b[0..32].try_into().unwrap()))
            .ok_or(DecodingError::InvalidAsk)?;
        let nsk = Option::from(jubjub::Fr::from_repr(b[32..64].try_into().unwrap()))
            .ok_or(DecodingError::InvalidNsk)?;
        let ovk = OutgoingViewingKey(b[64..96].try_into().unwrap());

        Ok(ExpandedSpendingKey { ask, nsk, ovk })
    }

    pub fn read<R: Read>(mut reader: R) -> io::Result<Self> {
        let mut repr = [0u8; 96];
        reader.read_exact(repr.as_mut())?;
        Self::from_bytes(&repr).map_err(|e| match e {
            DecodingError::InvalidAsk => {
                io::Error::new(io::ErrorKind::InvalidData, "ask not in field")
            }
            DecodingError::InvalidNsk => {
                io::Error::new(io::ErrorKind::InvalidData, "nsk not in field")
            }
            DecodingError::LengthInvalid { .. } => unreachable!(),
        })
    }

    pub fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        writer.write_all(&self.to_bytes())
    }

    /// Encodes the expanded spending key to the its seralized representation
    /// as part of the encoding of the extended spending key as defined in
    /// [ZIP 32](https://zips.z.cash/zip-0032)
    pub fn to_bytes(&self) -> [u8; 96] {
        let mut result = [0u8; 96];
        result[0..32].copy_from_slice(&self.ask.to_repr());
        result[32..64].copy_from_slice(&self.nsk.to_repr());
        result[64..96].copy_from_slice(&self.ovk.0);
        result
    }
}

impl Clone for FullViewingKey {
    fn clone(&self) -> Self {
        FullViewingKey {
            vk: ViewingKey {
                ak: self.vk.ak,
                nk: self.vk.nk,
            },
            ovk: self.ovk,
        }
    }
}

impl FullViewingKey {
    pub fn from_expanded_spending_key(expsk: &ExpandedSpendingKey) -> Self {
        FullViewingKey {
            vk: ViewingKey {
                ak: SPENDING_KEY_GENERATOR * expsk.ask,
                nk: NullifierDerivingKey(PROOF_GENERATION_KEY_GENERATOR * expsk.nsk),
            },
            ovk: expsk.ovk,
        }
    }

    pub fn read<R: Read>(mut reader: R) -> io::Result<Self> {
        let ak = {
            let mut buf = [0u8; 32];
            reader.read_exact(&mut buf)?;
            jubjub::SubgroupPoint::from_bytes(&buf).and_then(|p| CtOption::new(p, !p.is_identity()))
        };
        let nk = {
            let mut buf = [0u8; 32];
            reader.read_exact(&mut buf)?;
            jubjub::SubgroupPoint::from_bytes(&buf)
        };
        if ak.is_none().into() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "ak not of prime order",
            ));
        }
        if nk.is_none().into() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "nk not in prime-order subgroup",
            ));
        }
        let ak = ak.unwrap();
        let nk = NullifierDerivingKey(nk.unwrap());

        let mut ovk = [0u8; 32];
        reader.read_exact(&mut ovk)?;

        Ok(FullViewingKey {
            vk: ViewingKey { ak, nk },
            ovk: OutgoingViewingKey(ovk),
        })
    }

    pub fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        writer.write_all(&self.vk.ak.to_bytes())?;
        writer.write_all(&self.vk.nk.0.to_bytes())?;
        writer.write_all(&self.ovk.0)?;

        Ok(())
    }

    pub fn to_bytes(&self) -> [u8; 96] {
        let mut result = [0u8; 96];
        self.write(&mut result[..])
            .expect("should be able to serialize a FullViewingKey");
        result
    }
}

/// The scope of a viewing key or address.
///
/// A "scope" narrows the visibility or usage to a level below "full".
///
/// Consistent usage of `Scope` enables the user to provide consistent views over a wallet
/// to other people. For example, a user can give an external [`SaplingIvk`] to a merchant
/// terminal, enabling it to only detect "real" transactions from customers and not
/// internal transactions from the wallet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    /// A scope used for wallet-external operations, namely deriving addresses to give to
    /// other users in order to receive funds.
    External,
    /// A scope used for wallet-internal operations, such as creating change notes,
    /// auto-shielding, and note management.
    Internal,
}

/// A Sapling key that provides the capability to view incoming and outgoing transactions.
///
/// This key is useful anywhere you need to maintain accurate balance, but do not want the
/// ability to spend funds (such as a view-only wallet).
///
/// It comprises the subset of the ZIP 32 extended full viewing key that is used for the
/// Sapling item in a [ZIP 316 Unified Full Viewing Key][zip-0316-ufvk].
///
/// [zip-0316-ufvk]: https://zips.z.cash/zip-0316#encoding-of-unified-full-incoming-viewing-keys
#[derive(Clone, Debug)]
pub struct DiversifiableFullViewingKey {
    fvk: FullViewingKey,
    dk: zip32::DiversifierKey,
}

impl From<zip32::ExtendedFullViewingKey> for DiversifiableFullViewingKey {
    fn from(extfvk: zip32::ExtendedFullViewingKey) -> Self {
        Self {
            fvk: extfvk.fvk,
            dk: extfvk.dk,
        }
    }
}

impl DiversifiableFullViewingKey {
    /// Parses a `DiversifiableFullViewingKey` from its raw byte encoding.
    ///
    /// Returns `None` if the bytes do not contain a valid encoding of a diversifiable
    /// Sapling full viewing key.
    pub fn from_bytes(bytes: &[u8; 128]) -> Option<Self> {
        FullViewingKey::read(&bytes[..96]).ok().map(|fvk| Self {
            fvk,
            dk: zip32::DiversifierKey::from_bytes(bytes[96..].try_into().unwrap()),
        })
    }

    /// Returns the raw encoding of this `DiversifiableFullViewingKey`.
    pub fn to_bytes(&self) -> [u8; 128] {
        let mut bytes = [0; 128];
        self.fvk
            .write(&mut bytes[..96])
            .expect("slice should be the correct length");
        bytes[96..].copy_from_slice(&self.dk.as_bytes()[..]);
        bytes
    }

    /// Derives the internal `DiversifiableFullViewingKey` corresponding to `self` (which
    /// is assumed here to be an external DFVK).
    fn derive_internal(&self) -> Self {
        let (fvk, dk) = zip32::sapling_derive_internal_fvk(&self.fvk, &self.dk);
        Self { fvk, dk }
    }

    /// Exposes the external [`FullViewingKey`] component of this diversifiable full viewing key.
    pub fn fvk(&self) -> &FullViewingKey {
        &self.fvk
    }

    /// Derives a nullifier-deriving key for the provided scope.
    ///
    /// This API is provided so that nullifiers for change notes can be correctly computed.
    pub fn to_nk(&self, scope: Scope) -> NullifierDerivingKey {
        match scope {
            Scope::External => self.fvk.vk.nk,
            Scope::Internal => self.derive_internal().fvk.vk.nk,
        }
    }

    /// Derives an incoming viewing key corresponding to this full viewing key.
    pub fn to_ivk(&self, scope: Scope) -> SaplingIvk {
        match scope {
            Scope::External => self.fvk.vk.ivk(),
            Scope::Internal => self.derive_internal().fvk.vk.ivk(),
        }
    }

    /// Derives an outgoing viewing key corresponding to this full viewing key.
    pub fn to_ovk(&self, scope: Scope) -> OutgoingViewingKey {
        match scope {
            Scope::External => self.fvk.ovk,
            Scope::Internal => self.derive_internal().fvk.ovk,
        }
    }

    /// Attempts to produce a valid payment address for the given diversifier index.
    ///
    /// Returns `None` if the diversifier index does not produce a valid diversifier for
    /// this `DiversifiableFullViewingKey`.
    pub fn address(&self, j: zip32::DiversifierIndex) -> Option<PaymentAddress> {
        zip32::sapling_address(&self.fvk, &self.dk, j)
    }

    /// Finds the next valid payment address starting from the given diversifier index.
    ///
    /// This searches the diversifier space starting at `j` and incrementing, to find an
    /// index which will produce a valid diversifier (a 50% probability for each index).
    ///
    /// Returns the index at which the valid diversifier was found along with the payment
    /// address constructed using that diversifier, or `None` if the maximum index was
    /// reached and no valid diversifier was found.
    pub fn find_address(
        &self,
        j: zip32::DiversifierIndex,
    ) -> Option<(zip32::DiversifierIndex, PaymentAddress)> {
        zip32::sapling_find_address(&self.fvk, &self.dk, j)
    }

    /// Returns the payment address corresponding to the smallest valid diversifier index,
    /// along with that index.
    pub fn default_address(&self) -> (zip32::DiversifierIndex, PaymentAddress) {
        zip32::sapling_default_address(&self.fvk, &self.dk)
    }

    /// Returns the internal address corresponding to the smallest valid diversifier index,
    /// along with that index.
    ///
    /// This address **MUST NOT** be encoded and exposed to end users. User interfaces
    /// should instead mark these notes as "change notes" or "internal wallet operations".
    pub fn change_address(&self) -> (zip32::DiversifierIndex, PaymentAddress) {
        let internal_dfvk = self.derive_internal();
        zip32::sapling_default_address(&internal_dfvk.fvk, &internal_dfvk.dk)
    }

    /// Attempts to decrypt the given address's diversifier with this full viewing key.
    ///
    /// This method extracts the diversifier from the given address and decrypts it as a
    /// diversifier index, then verifies that this diversifier index produces the same
    /// address. Decryption is attempted using both the internal and external parts of the
    /// full viewing key.
    ///
    /// Returns the decrypted diversifier index and its scope, or `None` if the address
    /// was not generated from this key.
    pub fn decrypt_diversifier(
        &self,
        addr: &PaymentAddress,
    ) -> Option<(zip32::DiversifierIndex, Scope)> {
        let j_external = self.dk.diversifier_index(addr.diversifier());
        if self.address(j_external).as_ref() == Some(addr) {
            return Some((j_external, Scope::External));
        }

        let j_internal = self
            .derive_internal()
            .dk
            .diversifier_index(addr.diversifier());
        if self.address(j_internal).as_ref() == Some(addr) {
            return Some((j_internal, Scope::Internal));
        }

        None
    }
}

#[cfg(any(test, feature = "test-dependencies"))]
pub mod testing {
    use proptest::collection::vec;
    use proptest::prelude::{any, prop_compose};

    use crate::{
        sapling::PaymentAddress,
        zip32::{ExtendedFullViewingKey, ExtendedSpendingKey},
    };

    prop_compose! {
        pub fn arb_extended_spending_key()(v in vec(any::<u8>(), 32..252)) -> ExtendedSpendingKey {
            ExtendedSpendingKey::master(&v)
        }
    }

    prop_compose! {
        pub fn arb_shielded_addr()(extsk in arb_extended_spending_key()) -> PaymentAddress {
            let extfvk = ExtendedFullViewingKey::from(&extsk);
            extfvk.default_address().1
        }
    }
}

#[cfg(test)]
mod tests {
    use group::{Group, GroupEncoding};

    use super::{DiversifiableFullViewingKey, FullViewingKey};
    use crate::{constants::SPENDING_KEY_GENERATOR, zip32};

    #[test]
    fn ak_must_be_prime_order() {
        let mut buf = [0; 96];
        let identity = jubjub::SubgroupPoint::identity();

        // Set both ak and nk to the identity.
        buf[0..32].copy_from_slice(&identity.to_bytes());
        buf[32..64].copy_from_slice(&identity.to_bytes());

        // ak is not allowed to be the identity.
        assert_eq!(
            FullViewingKey::read(&buf[..]).unwrap_err().to_string(),
            "ak not of prime order"
        );

        // Set ak to a basepoint.
        let basepoint = SPENDING_KEY_GENERATOR;
        buf[0..32].copy_from_slice(&basepoint.to_bytes());

        // nk is allowed to be the identity.
        assert!(FullViewingKey::read(&buf[..]).is_ok());
    }

    #[test]
    fn dfvk_round_trip() {
        let dfvk = {
            let extsk = zip32::ExtendedSpendingKey::master(&[]);
            let extfvk = zip32::ExtendedFullViewingKey::from(&extsk);
            DiversifiableFullViewingKey::from(extfvk)
        };

        // Check value -> bytes -> parsed round trip.
        let dfvk_bytes = dfvk.to_bytes();
        let dfvk_parsed = DiversifiableFullViewingKey::from_bytes(&dfvk_bytes).unwrap();
        assert_eq!(dfvk_parsed.fvk.vk.ak, dfvk.fvk.vk.ak);
        assert_eq!(dfvk_parsed.fvk.vk.nk, dfvk.fvk.vk.nk);
        assert_eq!(dfvk_parsed.fvk.ovk, dfvk.fvk.ovk);
        assert_eq!(dfvk_parsed.dk, dfvk.dk);

        // Check bytes -> parsed -> bytes round trip.
        assert_eq!(dfvk_parsed.to_bytes(), dfvk_bytes);
    }
}
