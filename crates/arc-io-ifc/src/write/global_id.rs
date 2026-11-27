//! The IFC `GlobalId`: a 128-bit identifier written as 22 characters.
//!
//! The first character holds the two most significant bits and each following character the next
//! six bits, most significant first, in the published 64-character alphabet of IFC
//! (`0-9`, `A-Z`, `a-z`, `_`, `$`).

/// The 64 characters of the IFC `GlobalId` encoding, in the order of their values.
const ALPHABET: &[u8; 64] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz_$";

/// The 22-character `GlobalId` of a 128-bit identifier, such as the value of an `ObjectId`
/// (`arc_base::id::Identifier::as_u128`). The same identifier always gives the same text.
#[must_use]
pub fn global_id(uuid: u128) -> String {
    (0..22u32)
        .map(|position| {
            let shift = 126 - 6 * position;
            let value = (uuid >> shift) & 63;
            ALPHABET
                .get(usize::try_from(value).unwrap_or(0))
                .map_or('0', |&c| char::from(c))
        })
        .collect()
}

/// The SplitMix64 output function: a bijection of 64-bit values that spreads every input bit.
fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The role numbers of [`derived_id`]. The writer uses the roles below 256 for the objects it adds;
/// [`role::INSERT_OPENING`] is for the caller, and the roles from [`role::CALLER_FIRST`] up are never
/// used by the writer.
pub mod role {
    /// The `IfcRelAggregates` whose relating object is the owner.
    pub const AGGREGATES: u32 = 1;
    /// The `IfcRelContainedInSpatialStructure` of the owner storey.
    pub const CONTAINS: u32 = 2;
    /// The `IfcRelVoidsElement` of the owner opening.
    pub const VOIDS: u32 = 3;
    /// The `IfcRelFillsElement` of the owner door or window.
    pub const FILLS: u32 = 4;
    /// The `IfcPropertySet` at the index of the owner's property sets.
    pub const PROPERTY_SET: u32 = 5;
    /// The `IfcRelDefinesByProperties` of the property set at the index.
    pub const PROPERTY_REL: u32 = 6;
    /// The `IfcElementQuantity` at the index of the owner's quantity sets.
    pub const QUANTITY_SET: u32 = 7;
    /// The `IfcRelDefinesByProperties` of the quantity set at the index.
    pub const QUANTITY_REL: u32 = 8;
    /// The opening of an insert (a door or window) whose model object has no opening of its own:
    /// the caller derives the opening's identifier from the insert's with this role and index 0.
    pub const INSERT_OPENING: u32 = 9;
    /// The `IfcRelDefinesByType` of the owner element type.
    pub const TYPED: u32 = 10;
    /// The `IfcRelAssociatesMaterial` of the owner: an element type with its layer set, or an
    /// element with its layer set usage.
    pub const MATERIAL: u32 = 11;
    /// The `IfcRelAssignsToGroup` of the owner group.
    pub const GROUP: u32 = 12;
    /// The `IfcRelAssignsToActor` of the actor at the index of the owner's actors.
    pub const ACTOR: u32 = 13;
    /// The first role reserved for the caller's own derived identifiers.
    pub const CALLER_FIRST: u32 = 0x1_0000;
}

/// A stable identifier for an object derived from another object's identifier (`owner`), a role
/// ([`role`]) and an index. The same arguments always give the same identifier. It is a version 8
/// UUID, so it never equals a version 7 identifier such as an `ObjectId`; different arguments give
/// different identifiers except with negligible probability, and the writer refuses a duplicate
/// `GlobalId` in any case.
#[must_use]
pub fn derived_id(owner: u128, role: u32, index: u32) -> u128 {
    derived(owner, (u64::from(role) << 32) | u64::from(index))
}

fn derived(base: u128, role: u64) -> u128 {
    let high = u64::try_from(base >> 64).unwrap_or(0);
    let low = u64::try_from(base & u128::from(u64::MAX)).unwrap_or(0);
    let first = mix(high ^ mix(role));
    let second = mix(low ^ first.rotate_left(32) ^ role);
    let first = mix(first ^ second);
    let raw = (u128::from(first) << 64) | u128::from(second);
    // Version 8 in bits 76..80 and the variant `10` in bits 62..64.
    (raw & !(0xF << 76) & !(0b11 << 62)) | (0x8 << 76) | (0b10 << 62)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Vectors derived by hand from the encoding rule: two bits, then 21 groups of six.
    #[test]
    fn known_vectors() {
        assert_eq!(global_id(0), "0000000000000000000000");
        assert_eq!(global_id(1), "0000000000000000000001");
        assert_eq!(global_id(63), "000000000000000000000$");
        assert_eq!(global_id(64), "0000000000000000000010");
        assert_eq!(global_id(u128::MAX), "3$$$$$$$$$$$$$$$$$$$$$");
        assert_eq!(global_id(1 << 126), "1000000000000000000000");
        // Position i (1 to 21) holds the value i: 1 << 120 | 2 << 114 | ... | 21.
        assert_eq!(
            global_id(0x0108_3105_1872_0928_b30d_38f4_1149_3515),
            "0123456789ABCDEFGHIJKL"
        );
        // Two bits `10`, then `111111`, then zeros, then 5 in the last group.
        assert_eq!(
            global_id(0xbf00_0000_0000_0000_0000_0000_0000_0005),
            "2$00000000000000000005"
        );
    }

    #[test]
    fn derived_identifiers_are_stable_distinct_and_version_8() {
        let base = 0x0190_0000_0000_7000_8000_0000_0000_0001;
        let a = derived_id(base, role::VOIDS, 0);
        assert_eq!(a, derived(base, 3 << 32));
        assert_ne!(a, derived_id(base, role::VOIDS, 1));
        assert_ne!(a, derived_id(base, role::FILLS, 0));
        assert_ne!(a, derived_id(base + 1, role::VOIDS, 0));
        for id in [a, derived(0, 0), derived(u128::MAX, u64::MAX)] {
            assert_eq!((id >> 76) & 0xF, 8);
            assert_eq!((id >> 62) & 0b11, 0b10);
        }
    }
}
