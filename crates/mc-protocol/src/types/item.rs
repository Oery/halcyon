//! Server-authored item stacks. Vanilla defaults stay in the client registry;
//! only the supported per-stack presentation overrides are sent on the wire.
use std::rc::Rc;

use futures_lite::AsyncWriteExt;

use crate::data::items::ITEMS;
use crate::error::DecodeError;
use crate::packets::PacketWriter;
use crate::types::{Nbt, VarInt};

pub const MAX_TEXT_BYTES: usize = 256;
pub const MAX_LORE_LINES: usize = 16;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ItemComponents {
    pub damage: Option<u32>,
    /// None inherits the vanilla default; false explicitly removes the component.
    pub unbreakable: Option<bool>,
    pub name: Option<String>,
    pub lore: Vec<String>,
    pub dye: Option<u32>,
    pub glint: Option<bool>,
}

#[derive(Clone, Copy, Debug, thiserror::Error, PartialEq, Eq)]
pub enum ItemError {
    #[error("unknown item or air")]
    InvalidItem,
    #[error("item count exceeds its vanilla stack limit")]
    InvalidCount,
    #[error("invalid or excessive item component data")]
    InvalidComponents,
}

/// Empty stacks allocate nothing. Nondefault patches are shared by snapshots,
/// split stacks and equipment; count changes never copy text or encode patches.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ItemStack {
    item_id: u16,
    count: u8,
    components: Option<Rc<ItemComponents>>,
}

impl ItemStack {
    pub fn new(item_id: i32, count: u8, components: ItemComponents) -> Result<Self, ItemError> {
        let id = usize::try_from(item_id).map_err(|_| ItemError::InvalidItem)?;
        let (_, max_stack, max_damage, _) = ITEMS.get(id).ok_or(ItemError::InvalidItem)?;
        if id == 0 {
            return Err(ItemError::InvalidItem);
        }
        if count == 0 || count > *max_stack {
            return Err(ItemError::InvalidCount);
        }
        if components.damage.is_some_and(|damage| *max_damage == 0 || damage > *max_damage)
            || components.name.as_ref().is_some_and(|name| name.len() > MAX_TEXT_BYTES)
            || components.lore.len() > MAX_LORE_LINES
            || components.lore.iter().any(|line| line.len() > MAX_TEXT_BYTES)
            || components.dye.is_some_and(|rgb| rgb > 0x00ff_ffff)
            || components.body_len() > 4096
        {
            return Err(ItemError::InvalidComponents);
        }
        let components = (components != ItemComponents::default()).then(|| Rc::new(components));
        Ok(Self { item_id: id as u16, count, components })
    }

    pub fn plain(item_id: i32, count: u8) -> Result<Self, ItemError> {
        Self::new(item_id, count, ItemComponents::default())
    }

    pub fn item_id(&self) -> i32 {
        i32::from(self.item_id)
    }
    pub fn count(&self) -> u8 {
        self.count
    }
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
    pub fn components(&self) -> Option<&ItemComponents> {
        self.components.as_deref()
    }
    pub fn max_stack(&self) -> u8 {
        ITEMS[usize::from(self.item_id)].1
    }
    /// 0=ordinary, 1=feet, 2=legs, 3=chest, 4=head, 5=offhand.
    pub fn equipment_kind(&self) -> u8 {
        ITEMS[usize::from(self.item_id)].3
    }

    pub fn same_item(&self, other: &Self) -> bool {
        !self.is_empty()
            && !other.is_empty()
            && self.item_id == other.item_id
            && self.components == other.components
    }

    pub fn take(&mut self, amount: u8) -> Self {
        let amount = amount.min(self.count);
        if amount == 0 {
            return Self::default();
        }
        let taken = Self { count: amount, ..self.clone() };
        self.count -= amount;
        if self.count == 0 {
            *self = Self::default();
        }
        taken
    }

    /// Transfer up to amount into an empty/compatible destination. The caller
    /// supplies its slot restriction, while item stack limits remain mandatory.
    pub fn transfer_to(&mut self, target: &mut Self, amount: u8, slot_limit: u8) -> u8 {
        if self.is_empty() || (!target.is_empty() && !self.same_item(target)) {
            return 0;
        }
        let limit = self.max_stack().min(slot_limit);
        let moved = amount.min(self.count).min(limit.saturating_sub(target.count));
        if moved == 0 {
            return 0;
        }
        if target.is_empty() {
            *target = self.take(moved);
        } else {
            target.count += moved;
            self.take(moved);
        }
        moved
    }
}

impl ItemComponents {
    fn added_count(&self) -> i32 {
        self.damage.is_some() as i32
            + (self.unbreakable == Some(true)) as i32
            + self.name.is_some() as i32
            + (!self.lore.is_empty()) as i32
            + self.dye.is_some() as i32
            + self.glint.is_some() as i32
    }

    fn body_len(&self) -> usize {
        let mut len = VarInt(self.added_count()).body_len() + 1;
        if let Some(value) = self.damage {
            len += 1 + VarInt(value as i32).body_len();
        }
        if self.unbreakable == Some(true) {
            len += 1;
        }
        if self.unbreakable == Some(false) {
            len += 1;
        }
        if let Some(name) = &self.name {
            len += 1 + Nbt::Str(name).body_len();
        }
        if !self.lore.is_empty() {
            len += 1 + VarInt(self.lore.len() as i32).body_len();
            len += self.lore.iter().map(|line| Nbt::Str(line).body_len()).sum::<usize>();
        }
        if self.glint.is_some() {
            len += 2;
        }
        if self.dye.is_some() {
            len += 5;
        }
        len
    }

    async fn write<W: AsyncWriteExt + Unpin>(&self, w: &mut W) -> crate::encode::Result {
        VarInt(self.added_count()).write(w).await?;
        VarInt((self.unbreakable == Some(false)) as i32).write(w).await?;
        if let Some(value) = self.damage {
            VarInt(3).write(w).await?;
            VarInt(value as i32).write(w).await?;
        }
        if self.unbreakable == Some(true) {
            VarInt(4).write(w).await?;
        }
        if let Some(name) = &self.name {
            VarInt(6).write(w).await?;
            Nbt::Str(name).write(w).await?;
        }
        if !self.lore.is_empty() {
            VarInt(11).write(w).await?;
            VarInt(self.lore.len() as i32).write(w).await?;
            for line in &self.lore {
                Nbt::Str(line).write(w).await?;
            }
        }
        if let Some(glint) = self.glint {
            VarInt(21).write(w).await?;
            glint.write(w).await?;
        }
        if let Some(rgb) = self.dye {
            VarInt(47).write(w).await?;
            (rgb as i32).write(w).await?;
        }
        if self.unbreakable == Some(false) {
            VarInt(4).write(w).await?;
        }
        Ok(())
    }
}

impl PacketWriter<'_> for ItemStack {
    fn read(_input: &mut &[u8]) -> Result<Self, DecodeError> {
        // Survival actions transmit hashed predictions, never authoritative stacks.
        Err(DecodeError::EnumOutOfRange)
    }

    async fn write<W: AsyncWriteExt + Unpin>(&self, w: &mut W) -> crate::encode::Result {
        VarInt(i32::from(self.count)).write(w).await?;
        if self.is_empty() {
            return Ok(());
        }
        VarInt(self.item_id()).write(w).await?;
        if let Some(components) = &self.components {
            components.write(w).await?;
        } else {
            w.write_all(&[0, 0]).await?;
        }
        Ok(())
    }

    fn body_len(&self) -> usize {
        if self.is_empty() {
            return 1;
        }
        1 + VarInt(self.item_id()).body_len()
            + self.components.as_deref().map_or(2, ItemComponents::body_len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_limits_identity_and_component_wire_lengths() {
        let sword =
            ITEMS.iter().position(|item| item.0 == "minecraft:diamond_sword").unwrap() as i32;
        assert_eq!(ItemStack::plain(sword, 2).unwrap_err(), ItemError::InvalidCount);
        let mut item = ItemStack::new(
            sword,
            1,
            ItemComponents {
                damage: Some(5),
                unbreakable: Some(false),
                name: Some("Sword".into()),
                lore: vec!["Arena".into()],
                dye: Some(0x123456),
                glint: Some(true),
            },
        )
        .unwrap();
        let plain = ItemStack::plain(sword, 1).unwrap();
        assert!(!item.same_item(&plain));
        let bytes = futures_lite::future::block_on(async {
            let mut bytes = Vec::new();
            item.write(&mut bytes).await.unwrap();
            bytes
        });
        assert_eq!(item.body_len(), bytes.len());
        assert_eq!(&bytes[bytes.len() - 7..], &[1, 47, 0, 0x12, 0x34, 0x56, 4]);
        assert_eq!(item.take(1).count(), 1);
        assert!(item.is_empty());
        assert_eq!(item.body_len(), 1);
    }
}
