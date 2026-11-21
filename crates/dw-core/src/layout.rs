//! Byte offsets of the Unreal Engine structures the reflection walker reads.
//!
//! Defaults are for RuneScape: Dragonwilds on UE 5.6 (x86-64). They were
//! validated against a live dedicated server in Oct/2026; struct layout is the
//! same on Windows and Linux for these types. Keep them data, not constants, so
//! a future engine bump is a config change.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    // FChunkedFixedUObjectArray (the address handed to `Reflection::new` as `objects`)
    pub objects_chunks: u64,
    pub objects_max: u64,
    pub objects_num: u64,
    pub objects_max_chunks: u64,
    pub objects_num_chunks: u64,
    /// sizeof(FUObjectItem); the UObject* is its first field.
    pub object_item_size: u64,
    pub objects_per_chunk: u32,

    // FNamePool (the address handed to `Reflection::new` as `names`)
    pub names_current_block: u64,
    pub names_current_cursor: u64,
    pub names_blocks: u64,
    /// FNameEntry alignment; entry address = block + (index & 0xffff) * stride.
    pub name_stride: u64,

    // UObjectBase
    pub uobject_index: u64,
    pub uobject_class: u64,
    /// FName { ComparisonIndex: u32, Number: u32 }
    pub uobject_name: u64,
    pub uobject_outer: u64,

    // UField / UStruct / UFunction
    pub ufield_next: u64,
    pub ustruct_super: u64,
    pub ustruct_children: u64,
    pub ustruct_child_properties: u64,
    pub ustruct_size: u64,
    pub ufunction_flags: u64,
    pub ufunction_func: u64,

    // FField / FProperty. FFieldClass starts with its FName.
    pub ffield_class: u64,
    pub ffield_next: u64,
    pub ffield_name: u64,
    pub fproperty_element_size: u64,
    pub fproperty_offset: u64,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            objects_chunks: 0x0,
            objects_max: 0x10,
            objects_num: 0x14,
            objects_max_chunks: 0x18,
            objects_num_chunks: 0x1c,
            object_item_size: 0x18,
            objects_per_chunk: 65536,

            names_current_block: 0x38,
            names_current_cursor: 0x3c,
            names_blocks: 0x40,
            name_stride: 2,

            uobject_index: 0xc,
            uobject_class: 0x10,
            uobject_name: 0x18,
            uobject_outer: 0x20,

            ufield_next: 0x28,
            ustruct_super: 0x40,
            ustruct_children: 0x48,
            ustruct_child_properties: 0x50,
            ustruct_size: 0x58,
            ufunction_flags: 0xb0,
            ufunction_func: 0xd8,

            ffield_class: 0x8,
            ffield_next: 0x18,
            ffield_name: 0x20,
            fproperty_element_size: 0x34,
            fproperty_offset: 0x44,
        }
    }
}
