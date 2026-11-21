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
use crate::memory::Memory;
use serde::Serialize;

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
        Self { mem, layout, objects, names }
    }

    /// NumElements of the object array, if the header is sane
    /// (0 < num <= max <= 100_000_000, 0 < num_chunks <= max_chunks <= 2048,
    /// num_chunks * objects_per_chunk >= num).
    pub fn object_count(&self) -> Option<i32> {
        todo!()
    }

    /// The UObject* stored at `index`, or None for an empty slot / bad index.
    pub fn object_at(&self, index: i32) -> Option<u64> {
        todo!()
    }

    /// True when `obj` is 8-aligned, below 0x0000_7fff_ffff_f000, and the object
    /// array slot at its own InternalIndex points back at it.
    pub fn is_object(&self, obj: u64) -> bool {
        todo!()
    }

    /// Display string of an FName. Entry header (u16): length = hdr >> 6,
    /// wide = hdr & 1 (UTF-16LE when wide, Latin-1 otherwise); 0 < length < 1024.
    /// `number > 0` appends `_{number - 1}`. Rejects blocks past CurrentBlock and,
    /// in the current block, entries past CurrentByteCursor.
    pub fn name(&self, index: u32, number: u32) -> Option<String> {
        todo!()
    }

    /// FName of a UObject.
    pub fn object_name(&self, obj: u64) -> Option<String> {
        todo!()
    }

    /// ClassPrivate of a UObject (validated with `is_object`).
    pub fn class_of(&self, obj: u64) -> Option<u64> {
        todo!()
    }

    /// OuterPrivate of a UObject; None when it has no outer.
    pub fn outer_of(&self, obj: u64) -> Option<u64> {
        todo!()
    }

    /// Outer chain joined with '.', outermost first, e.g. "/Script/Dominion.Item".
    pub fn path_name(&self, obj: u64) -> Option<String> {
        todo!()
    }

    /// SuperStruct of a UStruct; None at the root.
    pub fn super_of(&self, ustruct: u64) -> Option<u64> {
        todo!()
    }

    /// First object whose class is named "Class" or "ScriptStruct" and whose own
    /// name equals `name` (short name, e.g. "HealthComponent").
    pub fn find_type(&self, name: &str) -> Option<u64> {
        todo!()
    }

    /// True when `obj`'s class, or any of its supers, has short name `class_name`.
    pub fn is_a(&self, obj: u64, class_name: &str) -> bool {
        todo!()
    }

    /// Properties declared directly on `ustruct` (ChildProperties chain), in order.
    pub fn properties(&self, ustruct: u64) -> Vec<PropertyInfo> {
        todo!()
    }

    /// Property `name` on `ustruct` or the nearest super that declares it.
    pub fn find_property(&self, ustruct: u64, name: &str) -> Option<PropertyInfo> {
        todo!()
    }

    /// Functions declared directly on `class` (Children chain, objects whose class is "Function").
    /// `native_base` is subtracted from native addresses (pass 0 for absolute).
    pub fn functions(&self, class: u64, native_base: u64) -> Vec<FunctionInfo> {
        todo!()
    }

    /// Function `name` on `class` or the nearest super that declares it.
    pub fn find_function(&self, class: u64, name: &str) -> Option<FunctionInfo> {
        todo!()
    }

    /// Every live object that `is_a(class_name)`, skipping class default objects
    /// (names starting with "Default__").
    pub fn instances_of(&self, class_name: &str) -> Vec<u64> {
        todo!()
    }

    /// Every UClass / UScriptStruct whose short name passes `filter`, in object-array order.
    pub fn dump(&self, filter: &dyn Fn(&str) -> bool, native_base: u64) -> Vec<TypeDump> {
        todo!()
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
