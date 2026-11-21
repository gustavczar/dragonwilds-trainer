//! Unreal Engine 5.6 reflection walker: objects, names, types, properties and
//! functions, all resolved by name through a [`Memory`].
//!
//! Rules every method follows:
//! - never panic and never loop forever on corrupt memory: every walk is bounded
//!   (at most 64 links up a super/outer chain, at most 4096 fields per struct);
//! - a failed read yields `None` / `false` / an empty `Vec`, never a guess;
//! - names are compared exactly (case-sensitive), as the engine does for FName
//!   display strings.

use crate::layout::Layout;
use crate::memory::{Memory, MemoryExt};
use serde::Serialize;
use std::collections::HashSet;

/// UFunction flag: the function has a native (C++) implementation.
pub const FUNC_NATIVE: u32 = 0x400;

/// A reflected property (FProperty).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PropertyInfo {
    pub name: String,
    /// FFieldClass name, e.g. "IntProperty", "MapProperty".
    #[serde(rename = "type")]
    pub type_name: String,
    /// Byte offset inside the owning struct/object.
    #[serde(serialize_with = "hex_i32")]
    pub offset: i32,
    /// FProperty::ElementSize.
    #[serde(serialize_with = "hex_i32")]
    pub size: i32,
}

/// A reflected function (UFunction).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FunctionInfo {
    pub name: String,
    /// Address of the UFunction object itself (needed to call it via ProcessEvent).
    #[serde(skip)]
    pub object: u64,
    #[serde(serialize_with = "hex_u32")]
    pub flags: u32,
    /// Native implementation address minus `native_base` when FUNC_NATIVE is set, else None.
    #[serde(serialize_with = "hex_opt_u64")]
    pub native: Option<u64>,
    /// Parameters, in declaration order (the UFunction's own properties).
    pub params: Vec<PropertyInfo>,
}

/// One UClass or UScriptStruct, in the JSON schema of the earlier dumps
/// (`{"name","kind","super","size","props","functions"}`, numbers as "0x..").
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TypeDump {
    pub name: String,
    /// "Class" or "ScriptStruct".
    pub kind: String,
    #[serde(rename = "super")]
    pub super_name: Option<String>,
    #[serde(serialize_with = "hex_i32")]
    pub size: i32,
    pub props: Vec<PropertyInfo>,
    /// Empty for ScriptStruct; omitted from JSON when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub functions: Vec<FunctionInfo>,
}

/// Reflection view over one game process.
pub struct Reflection<'m, M: Memory + ?Sized> {
    pub mem: &'m M,
    pub layout: Layout,
    /// Address of FChunkedFixedUObjectArray (GUObjectArray.ObjObjects).
    pub objects: u64,
    /// Address of the FNamePool.
    pub names: u64,
}

impl<'m, M: Memory + ?Sized> Reflection<'m, M> {
    pub fn new(mem: &'m M, layout: Layout, objects: u64, names: u64) -> Self {
        Self {
            mem,
            layout,
            objects,
            names,
        }
    }

    /// NumElements of the object array, if the header is sane
    /// (0 < num <= max <= 100_000_000, 0 < num_chunks <= max_chunks <= 2048,
    /// num_chunks * objects_per_chunk >= num).
    pub fn object_count(&self) -> Option<i32> {
        let chunks = self
            .mem
            .read_u64(self.objects.checked_add(self.layout.objects_chunks)?)?;
        let max = self
            .mem
            .read_i32(self.objects.checked_add(self.layout.objects_max)?)?;
        let num = self
            .mem
            .read_i32(self.objects.checked_add(self.layout.objects_num)?)?;
        let max_chunks = self
            .mem
            .read_i32(self.objects.checked_add(self.layout.objects_max_chunks)?)?;
        let num_chunks = self
            .mem
            .read_i32(self.objects.checked_add(self.layout.objects_num_chunks)?)?;

        if chunks == 0
            || num <= 0
            || num > max
            || max > 100_000_000
            || num_chunks <= 0
            || num_chunks > max_chunks
            || max_chunks > 2048
            || self.layout.objects_per_chunk == 0
        {
            return None;
        }

        let capacity = u64::try_from(num_chunks)
            .ok()?
            .checked_mul(u64::from(self.layout.objects_per_chunk))?;
        (capacity >= u64::try_from(num).ok()?).then_some(num)
    }

    /// The UObject* stored at `index`, or None for an empty slot / bad index.
    pub fn object_at(&self, index: i32) -> Option<u64> {
        let count = self.object_count()?;
        if index < 0 || index >= count {
            return None;
        }

        let index = u64::try_from(index).ok()?;
        let per_chunk = u64::from(self.layout.objects_per_chunk);
        let chunks = self
            .mem
            .read_u64(self.objects.checked_add(self.layout.objects_chunks)?)?;
        let chunk_offset = (index / per_chunk).checked_mul(8)?;
        let chunk = self.mem.read_u64(chunks.checked_add(chunk_offset)?)?;
        if chunk == 0 {
            return None;
        }
        let item_offset = (index % per_chunk).checked_mul(self.layout.object_item_size)?;
        let object = self.mem.read_u64(chunk.checked_add(item_offset)?)?;
        (object != 0).then_some(object)
    }

    /// True when `obj` is 8-aligned, below 0x0000_7fff_ffff_f000, and the object
    /// array slot at its own InternalIndex points back at it.
    pub fn is_object(&self, obj: u64) -> bool {
        if obj == 0 || obj & 7 != 0 || obj >= 0x0000_7fff_ffff_f000 {
            return false;
        }
        let Some(index_addr) = obj.checked_add(self.layout.uobject_index) else {
            return false;
        };
        let Some(index) = self.mem.read_i32(index_addr) else {
            return false;
        };
        index >= 0 && self.object_at(index) == Some(obj)
    }

    /// Display string of an FName. Entry header (u16): length = hdr >> 6,
    /// wide = hdr & 1 (UTF-16LE when wide, Latin-1 otherwise); 0 < length < 1024.
    /// `number > 0` appends `_{number - 1}`. Rejects blocks past CurrentBlock and,
    /// in the current block, entries past CurrentByteCursor.
    pub fn name(&self, index: u32, number: u32) -> Option<String> {
        let current_block = self
            .mem
            .read_u32(self.names.checked_add(self.layout.names_current_block)?)?;
        let cursor = self
            .mem
            .read_u32(self.names.checked_add(self.layout.names_current_cursor)?)?;
        if current_block >= 8192 || cursor > 0x20_000 {
            return None;
        }

        let block = index >> 16;
        if block > current_block {
            return None;
        }
        let offset = u64::from(index & 0xffff).checked_mul(self.layout.name_stride)?;
        let block_offset = u64::from(block).checked_mul(8)?;
        let block_slot = self
            .names
            .checked_add(self.layout.names_blocks)?
            .checked_add(block_offset)?;
        let block_address = self.mem.read_u64(block_slot)?;
        if block_address == 0 {
            return None;
        }
        let entry = block_address.checked_add(offset)?;
        let header = self.mem.read_u16(entry)?;
        let length = u64::from(header >> 6);
        if length == 0 || length >= 1024 {
            return None;
        }
        let wide = header & 1 != 0;
        let byte_length = length.checked_mul(if wide { 2 } else { 1 })?;
        if block == current_block {
            let end = offset.checked_add(2)?.checked_add(byte_length)?;
            if end > u64::from(cursor) {
                return None;
            }
        }

        let mut bytes = vec![0; usize::try_from(byte_length).ok()?];
        let text_address = entry.checked_add(2)?;
        if !self.mem.read(text_address, &mut bytes) {
            return None;
        }
        let mut text = if wide {
            let units = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>();
            String::from_utf16(&units).ok()?
        } else {
            bytes.into_iter().map(char::from).collect()
        };
        if number > 0 {
            text.push('_');
            text.push_str(&(number - 1).to_string());
        }
        Some(text)
    }

    /// FName of a UObject.
    pub fn object_name(&self, obj: u64) -> Option<String> {
        self.fname_at(obj.checked_add(self.layout.uobject_name)?)
    }

    /// ClassPrivate of a UObject (validated with `is_object`).
    pub fn class_of(&self, obj: u64) -> Option<u64> {
        let class = self
            .mem
            .read_u64(obj.checked_add(self.layout.uobject_class)?)?;
        self.is_object(class).then_some(class)
    }

    /// OuterPrivate of a UObject; None when it has no outer.
    pub fn outer_of(&self, obj: u64) -> Option<u64> {
        let outer = self
            .mem
            .read_u64(obj.checked_add(self.layout.uobject_outer)?)?;
        (outer != 0).then_some(outer)
    }

    /// Outer chain joined with '.', outermost first, e.g. "/Script/Dominion.Item".
    pub fn path_name(&self, obj: u64) -> Option<String> {
        let mut current = obj;
        let mut seen = HashSet::new();
        let mut parts = Vec::new();

        for _ in 0..64 {
            if !seen.insert(current) || !self.is_object(current) {
                return None;
            }
            parts.push(self.object_name(current)?);
            let outer = self
                .mem
                .read_u64(current.checked_add(self.layout.uobject_outer)?)?;
            if outer == 0 {
                parts.reverse();
                return Some(parts.join("."));
            }
            current = outer;
        }
        None
    }

    /// SuperStruct of a UStruct; None at the root.
    pub fn super_of(&self, ustruct: u64) -> Option<u64> {
        let super_struct = self
            .mem
            .read_u64(ustruct.checked_add(self.layout.ustruct_super)?)?;
        (super_struct != 0).then_some(super_struct)
    }

    /// First object whose class is named "Class" or "ScriptStruct" and whose own
    /// name equals `name` (short name, e.g. "HealthComponent").
    pub fn find_type(&self, name: &str) -> Option<u64> {
        let count = self.object_count()?;
        for index in 0..count {
            let Some(object) = self.object_at(index) else {
                continue;
            };
            let Some(class) = self.class_of(object) else {
                continue;
            };
            let Some(kind) = self.object_name(class) else {
                continue;
            };
            if matches!(kind.as_str(), "Class" | "ScriptStruct")
                && self.object_name(object).as_deref() == Some(name)
            {
                return Some(object);
            }
        }
        None
    }

    /// True when `obj`'s class, or any of its supers, has short name `class_name`.
    pub fn is_a(&self, obj: u64, class_name: &str) -> bool {
        let Some(mut current) = self.class_of(obj) else {
            return false;
        };
        let mut seen = HashSet::new();
        for _ in 0..64 {
            if !seen.insert(current) || !self.is_object(current) {
                return false;
            }
            if self.object_name(current).as_deref() == Some(class_name) {
                return true;
            }
            let Some(next) = self.super_of(current) else {
                return false;
            };
            current = next;
        }
        false
    }

    /// Properties declared directly on `ustruct` (ChildProperties chain), in order.
    pub fn properties(&self, ustruct: u64) -> Vec<PropertyInfo> {
        let Some(head_addr) = ustruct.checked_add(self.layout.ustruct_child_properties) else {
            return Vec::new();
        };
        let Some(mut field) = self.mem.read_u64(head_addr) else {
            return Vec::new();
        };
        let mut seen = HashSet::new();
        let mut properties = Vec::new();

        for _ in 0..4096 {
            if field == 0 || !seen.insert(field) {
                break;
            }
            let Some(property) = self.property_info(field) else {
                break;
            };
            properties.push(property);
            let Some(next_addr) = field.checked_add(self.layout.ffield_next) else {
                break;
            };
            let Some(next) = self.mem.read_u64(next_addr) else {
                break;
            };
            field = next;
        }
        properties
    }

    /// Property `name` on `ustruct` or the nearest super that declares it.
    pub fn find_property(&self, ustruct: u64, name: &str) -> Option<PropertyInfo> {
        let mut current = ustruct;
        let mut seen = HashSet::new();
        for _ in 0..64 {
            if !seen.insert(current) {
                return None;
            }
            if let Some(property) = self
                .properties(current)
                .into_iter()
                .find(|property| property.name == name)
            {
                return Some(property);
            }
            current = self.super_of(current)?;
        }
        None
    }

    /// Functions declared directly on `class` (Children chain, objects whose class is "Function").
    /// `native_base` is subtracted from native addresses (pass 0 for absolute).
    pub fn functions(&self, class: u64, native_base: u64) -> Vec<FunctionInfo> {
        let Some(head_addr) = class.checked_add(self.layout.ustruct_children) else {
            return Vec::new();
        };
        let Some(mut child) = self.mem.read_u64(head_addr) else {
            return Vec::new();
        };
        let mut seen = HashSet::new();
        let mut functions = Vec::new();

        for _ in 0..4096 {
            if child == 0 || !seen.insert(child) {
                break;
            }
            let next = child
                .checked_add(self.layout.ufield_next)
                .and_then(|address| self.mem.read_u64(address));

            let is_function = self
                .class_of(child)
                .and_then(|object_class| self.object_name(object_class))
                .as_deref()
                == Some("Function");
            if is_function {
                let info = (|| {
                    let name = self.object_name(child)?;
                    let flags = self
                        .mem
                        .read_u32(child.checked_add(self.layout.ufunction_flags)?)?;
                    let implementation = self
                        .mem
                        .read_u64(child.checked_add(self.layout.ufunction_func)?)?;
                    let native = if flags & FUNC_NATIVE != 0 && implementation != 0 {
                        implementation.checked_sub(native_base)
                    } else {
                        None
                    };
                    Some(FunctionInfo {
                        name,
                        object: child,
                        flags,
                        native,
                        params: self.properties(child),
                    })
                })();
                if let Some(info) = info {
                    functions.push(info);
                }
            }

            let Some(next) = next else {
                break;
            };
            child = next;
        }
        functions
    }

    /// Function `name` on `class` or the nearest super that declares it.
    pub fn find_function(&self, class: u64, name: &str) -> Option<FunctionInfo> {
        let mut current = class;
        let mut seen = HashSet::new();
        for _ in 0..64 {
            if !seen.insert(current) {
                return None;
            }
            if let Some(function) = self
                .functions(current, 0)
                .into_iter()
                .find(|function| function.name == name)
            {
                return Some(function);
            }
            current = self.super_of(current)?;
        }
        None
    }

    /// Every live object that `is_a(class_name)`, skipping class default objects
    /// (names starting with "Default__").
    pub fn instances_of(&self, class_name: &str) -> Vec<u64> {
        let Some(count) = self.object_count() else {
            return Vec::new();
        };
        let mut instances = Vec::new();
        for index in 0..count {
            let Some(object) = self.object_at(index) else {
                continue;
            };
            if !self.is_a(object, class_name) {
                continue;
            }
            let Some(name) = self.object_name(object) else {
                continue;
            };
            if !name.starts_with("Default__") {
                instances.push(object);
            }
        }
        instances
    }

    /// Every UClass / UScriptStruct whose short name passes `filter`, in object-array order.
    pub fn dump(&self, filter: &dyn Fn(&str) -> bool, native_base: u64) -> Vec<TypeDump> {
        let Some(count) = self.object_count() else {
            return Vec::new();
        };
        let mut dumps = Vec::new();
        for index in 0..count {
            let Some(object) = self.object_at(index) else {
                continue;
            };
            let Some(object_class) = self.class_of(object) else {
                continue;
            };
            let Some(kind) = self.object_name(object_class) else {
                continue;
            };
            if !matches!(kind.as_str(), "Class" | "ScriptStruct") {
                continue;
            }
            let Some(name) = self.object_name(object) else {
                continue;
            };
            if !filter(&name) {
                continue;
            }
            let Some(size) = object
                .checked_add(self.layout.ustruct_size)
                .and_then(|address| self.mem.read_i32(address))
            else {
                continue;
            };
            let Some(super_address) = object.checked_add(self.layout.ustruct_super) else {
                continue;
            };
            let Some(super_struct) = self.mem.read_u64(super_address) else {
                continue;
            };
            let super_name = if super_struct == 0 {
                None
            } else {
                let Some(name) = self.object_name(super_struct) else {
                    continue;
                };
                Some(name)
            };
            let functions = if kind == "Class" {
                self.functions(object, native_base)
            } else {
                Vec::new()
            };
            dumps.push(TypeDump {
                name,
                kind,
                super_name,
                size,
                props: self.properties(object),
                functions,
            });
        }
        dumps
    }

    fn fname_at(&self, address: u64) -> Option<String> {
        let index = self.mem.read_u32(address)?;
        let number = self.mem.read_u32(address.checked_add(4)?)?;
        self.name(index, number)
    }

    fn property_info(&self, field: u64) -> Option<PropertyInfo> {
        let name = self.fname_at(field.checked_add(self.layout.ffield_name)?)?;
        let field_class = self
            .mem
            .read_u64(field.checked_add(self.layout.ffield_class)?)?;
        if field_class == 0 {
            return None;
        }
        let type_name = self.fname_at(field_class)?;
        let offset = self
            .mem
            .read_i32(field.checked_add(self.layout.fproperty_offset)?)?;
        let size = self
            .mem
            .read_i32(field.checked_add(self.layout.fproperty_element_size)?)?;
        Some(PropertyInfo {
            name,
            type_name,
            offset,
            size,
        })
    }
}

fn hex_i32<S: serde::Serializer>(v: &i32, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&format!("{v:#x}"))
}
fn hex_u32<S: serde::Serializer>(v: &u32, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&format!("{v:#x}"))
}
fn hex_opt_u64<S: serde::Serializer>(v: &Option<u64>, s: S) -> Result<S::Ok, S::Error> {
    match v {
        Some(v) => s.serialize_str(&format!("{v:#x}")),
        None => s.serialize_none(),
    }
}
