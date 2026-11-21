use dw_core::{Layout, Memory, Reflection};
use std::collections::BTreeMap;

const OBJECTS: u64 = 0x1000;
const CHUNKS: u64 = 0x2000;
const CHUNK: u64 = 0x3000;
const NAMES: u64 = 0x10_000;
const NAME_BLOCK: u64 = 0x11_000;
const EXTRA_NAME_BLOCK: u64 = 0x13_000;

const CLASS_META: u64 = 0x20_000;
const SCRIPT_STRUCT_META: u64 = 0x21_000;
const FUNCTION_META: u64 = 0x22_000;
const PACKAGE: u64 = 0x23_000;
const OBJECT_CLASS: u64 = 0x24_000;
const ACTOR_COMPONENT: u64 = 0x25_000;
const HEALTH_COMPONENT: u64 = 0x26_000;
const STAT_DATA: u64 = 0x27_000;
const SET_IMMORTAL: u64 = 0x28_000;
const ACTIVATE: u64 = 0x29_000;
const HEALTH_INSTANCE: u64 = 0x2a_000;
const HEALTH_CDO: u64 = 0x2b_000;
const NOT_IN_ARRAY: u64 = 0x2c_000;

const FLOAT_FIELD_CLASS: u64 = 0x40_000;
const BOOL_FIELD_CLASS: u64 = 0x40_100;
const MAX_HEALTH_FIELD: u64 = 0x41_000;
const CAN_DIE_FIELD: u64 = 0x41_100;
const IMMORTAL_PARAM: u64 = 0x41_200;

#[derive(Clone, Default)]
struct FakeMemory {
    regions: BTreeMap<u64, Vec<u8>>,
}

impl FakeMemory {
    fn region(&mut self, addr: u64, size: usize) {
        assert!(self.regions.insert(addr, vec![0; size]).is_none());
    }

    fn write(&mut self, addr: u64, bytes: &[u8]) {
        let (&base, region) = self
            .regions
            .range_mut(..=addr)
            .next_back()
            .expect("write address has a backing region");
        let start = usize::try_from(addr - base).unwrap();
        let end = start.checked_add(bytes.len()).unwrap();
        assert!(end <= region.len(), "write crosses a region boundary");
        region[start..end].copy_from_slice(bytes);
    }

    fn u16(&mut self, addr: u64, value: u16) {
        self.write(addr, &value.to_le_bytes());
    }

    fn u32(&mut self, addr: u64, value: u32) {
        self.write(addr, &value.to_le_bytes());
    }

    fn i32(&mut self, addr: u64, value: i32) {
        self.write(addr, &value.to_le_bytes());
    }

    fn u64(&mut self, addr: u64, value: u64) {
        self.write(addr, &value.to_le_bytes());
    }
}

impl Memory for FakeMemory {
    fn read(&self, addr: u64, out: &mut [u8]) -> bool {
        let Some((&base, region)) = self.regions.range(..=addr).next_back() else {
            return false;
        };
        let Ok(start) = usize::try_from(addr - base) else {
            return false;
        };
        let Some(end) = start.checked_add(out.len()) else {
            return false;
        };
        if end > region.len() {
            return false;
        }
        out.copy_from_slice(&region[start..end]);
        true
    }
}

#[derive(Clone, Copy)]
struct NameIds {
    class: u32,
    script_struct: u32,
    function: u32,
    package: u32,
    object: u32,
    actor_component: u32,
    health_component: u32,
    stat_data: u32,
    set_immortal: u32,
    activate: u32,
    max_health: u32,
    can_die: u32,
    immortal: u32,
    default_health: u32,
    wide: u32,
    hidden: u32,
}

struct Image {
    mem: FakeMemory,
    layout: Layout,
    names: NameIds,
}

impl Image {
    fn reflection(&self) -> Reflection<'_, FakeMemory> {
        Reflection::new(&self.mem, self.layout, OBJECTS, NAMES)
    }
}

fn add_latin(mem: &mut FakeMemory, cursor: &mut u32, text: &str) -> u32 {
    let offset = *cursor;
    let bytes: Vec<u8> = text
        .chars()
        .map(|ch| u8::try_from(u32::from(ch)).expect("Latin-1 test name"))
        .collect();
    mem.u16(NAME_BLOCK + u64::from(offset), (bytes.len() as u16) << 6);
    mem.write(NAME_BLOCK + u64::from(offset) + 2, &bytes);
    *cursor = (offset + 2 + bytes.len() as u32 + 1) & !1;
    offset / 2
}

fn add_wide(mem: &mut FakeMemory, cursor: &mut u32, text: &str) -> u32 {
    let offset = *cursor;
    let units: Vec<u16> = text.encode_utf16().collect();
    mem.u16(
        NAME_BLOCK + u64::from(offset),
        ((units.len() as u16) << 6) | 1,
    );
    let bytes: Vec<u8> = units.into_iter().flat_map(u16::to_le_bytes).collect();
    mem.write(NAME_BLOCK + u64::from(offset) + 2, &bytes);
    *cursor = (offset + 2 + bytes.len() as u32 + 1) & !1;
    offset / 2
}

fn write_fname(mem: &mut FakeMemory, addr: u64, index: u32, number: u32) {
    mem.u32(addr, index);
    mem.u32(addr + 4, number);
}

fn write_object(
    mem: &mut FakeMemory,
    layout: Layout,
    index: i32,
    addr: u64,
    class: u64,
    name: (u32, u32),
    outer: u64,
) {
    mem.region(addr, 0x200);
    mem.i32(addr + layout.uobject_index, index);
    mem.u64(addr + layout.uobject_class, class);
    write_fname(mem, addr + layout.uobject_name, name.0, name.1);
    mem.u64(addr + layout.uobject_outer, outer);
    mem.u64(CHUNK + (index as u64) * layout.object_item_size, addr);
}

fn synthetic_image() -> Image {
    let layout = Layout::default();
    let mut mem = FakeMemory::default();
    mem.region(OBJECTS, 0x100);
    mem.region(CHUNKS, 8);
    mem.region(CHUNK, 0x400);
    mem.region(NAMES, 0x100);
    mem.region(NAME_BLOCK, 0x2000);
    mem.region(EXTRA_NAME_BLOCK, 0x100);

    mem.u64(OBJECTS + layout.objects_chunks, CHUNKS);
    mem.i32(OBJECTS + layout.objects_max, 16);
    mem.i32(OBJECTS + layout.objects_num, 12);
    mem.i32(OBJECTS + layout.objects_max_chunks, 1);
    mem.i32(OBJECTS + layout.objects_num_chunks, 1);
    mem.u64(CHUNKS, CHUNK);

    mem.u32(NAMES + layout.names_current_block, 0);
    mem.u64(NAMES + layout.names_blocks, NAME_BLOCK);
    mem.u64(NAMES + layout.names_blocks + 8, EXTRA_NAME_BLOCK);

    let mut cursor = 0;
    let names = NameIds {
        class: add_latin(&mut mem, &mut cursor, "Class"),
        script_struct: add_latin(&mut mem, &mut cursor, "ScriptStruct"),
        function: add_latin(&mut mem, &mut cursor, "Function"),
        package: add_latin(&mut mem, &mut cursor, "/Script/Dominion"),
        object: add_latin(&mut mem, &mut cursor, "Object"),
        actor_component: add_latin(&mut mem, &mut cursor, "ActorComponent"),
        health_component: add_latin(&mut mem, &mut cursor, "HealthComponent"),
        stat_data: add_latin(&mut mem, &mut cursor, "StatData"),
        set_immortal: add_latin(&mut mem, &mut cursor, "SetIsImmortal"),
        activate: add_latin(&mut mem, &mut cursor, "Activate"),
        max_health: add_latin(&mut mem, &mut cursor, "MaxHealth"),
        can_die: add_latin(&mut mem, &mut cursor, "bCanDie"),
        immortal: add_latin(&mut mem, &mut cursor, "bImmortal"),
        default_health: add_latin(&mut mem, &mut cursor, "Default__HealthComponent"),
        wide: add_wide(&mut mem, &mut cursor, "生命"),
        hidden: 0,
    };
    let visible_cursor = cursor;
    let hidden_offset = visible_cursor + 0x20;
    let hidden = hidden_offset / 2;
    mem.u16(NAME_BLOCK + u64::from(hidden_offset), 6 << 6);
    mem.write(NAME_BLOCK + u64::from(hidden_offset) + 2, b"Hidden");
    mem.u16(EXTRA_NAME_BLOCK, 5 << 6);
    mem.write(EXTRA_NAME_BLOCK + 2, b"Other");
    mem.u32(NAMES + layout.names_current_cursor, visible_cursor);
    let names = NameIds { hidden, ..names };

    write_object(
        &mut mem,
        layout,
        0,
        CLASS_META,
        CLASS_META,
        (names.class, 0),
        0,
    );
    write_object(
        &mut mem,
        layout,
        1,
        SCRIPT_STRUCT_META,
        CLASS_META,
        (names.script_struct, 0),
        0,
    );
    write_object(
        &mut mem,
        layout,
        2,
        FUNCTION_META,
        CLASS_META,
        (names.function, 0),
        0,
    );
    write_object(
        &mut mem,
        layout,
        3,
        PACKAGE,
        OBJECT_CLASS,
        (names.package, 0),
        0,
    );
    write_object(
        &mut mem,
        layout,
        4,
        OBJECT_CLASS,
        CLASS_META,
        (names.object, 0),
        PACKAGE,
    );
    write_object(
        &mut mem,
        layout,
        5,
        ACTOR_COMPONENT,
        CLASS_META,
        (names.actor_component, 0),
        PACKAGE,
    );
    write_object(
        &mut mem,
        layout,
        6,
        HEALTH_COMPONENT,
        CLASS_META,
        (names.health_component, 0),
        PACKAGE,
    );
    write_object(
        &mut mem,
        layout,
        7,
        STAT_DATA,
        SCRIPT_STRUCT_META,
        (names.stat_data, 0),
        PACKAGE,
    );
    write_object(
        &mut mem,
        layout,
        8,
        SET_IMMORTAL,
        FUNCTION_META,
        (names.set_immortal, 0),
        HEALTH_COMPONENT,
    );
    write_object(
        &mut mem,
        layout,
        9,
        ACTIVATE,
        FUNCTION_META,
        (names.activate, 0),
        ACTOR_COMPONENT,
    );
    write_object(
        &mut mem,
        layout,
        10,
        HEALTH_INSTANCE,
        HEALTH_COMPONENT,
        (names.health_component, 1),
        0,
    );
    write_object(
        &mut mem,
        layout,
        11,
        HEALTH_CDO,
        HEALTH_COMPONENT,
        (names.default_health, 0),
        0,
    );

    mem.region(NOT_IN_ARRAY, 0x100);
    mem.i32(NOT_IN_ARRAY + layout.uobject_index, 0);

    mem.u64(OBJECT_CLASS + layout.ustruct_super, 0);
    mem.u64(ACTOR_COMPONENT + layout.ustruct_super, OBJECT_CLASS);
    mem.u64(HEALTH_COMPONENT + layout.ustruct_super, ACTOR_COMPONENT);
    mem.u64(STAT_DATA + layout.ustruct_super, 0);
    mem.i32(OBJECT_CLASS + layout.ustruct_size, 0x30);
    mem.i32(ACTOR_COMPONENT + layout.ustruct_size, 0x140);
    mem.i32(HEALTH_COMPONENT + layout.ustruct_size, 0x150);
    mem.i32(STAT_DATA + layout.ustruct_size, 0x20);
    mem.region(FLOAT_FIELD_CLASS, 0x80);
    mem.region(BOOL_FIELD_CLASS, 0x80);
    let float_property = add_latin(&mut mem, &mut cursor, "FloatProperty");
    let bool_property = add_latin(&mut mem, &mut cursor, "BoolProperty");
    write_fname(&mut mem, FLOAT_FIELD_CLASS, float_property, 0);
    write_fname(&mut mem, BOOL_FIELD_CLASS, bool_property, 0);
    mem.u32(NAMES + layout.names_current_cursor, cursor);

    for addr in [MAX_HEALTH_FIELD, CAN_DIE_FIELD, IMMORTAL_PARAM] {
        mem.region(addr, 0x80);
    }
    mem.u64(MAX_HEALTH_FIELD + layout.ffield_class, FLOAT_FIELD_CLASS);
    write_fname(
        &mut mem,
        MAX_HEALTH_FIELD + layout.ffield_name,
        names.max_health,
        0,
    );
    mem.i32(MAX_HEALTH_FIELD + layout.fproperty_offset, 0x138);
    mem.i32(MAX_HEALTH_FIELD + layout.fproperty_element_size, 4);

    mem.u64(CAN_DIE_FIELD + layout.ffield_class, BOOL_FIELD_CLASS);
    write_fname(
        &mut mem,
        CAN_DIE_FIELD + layout.ffield_name,
        names.can_die,
        0,
    );
    mem.i32(CAN_DIE_FIELD + layout.fproperty_offset, 0x140);
    mem.i32(CAN_DIE_FIELD + layout.fproperty_element_size, 1);

    mem.u64(IMMORTAL_PARAM + layout.ffield_class, BOOL_FIELD_CLASS);
    write_fname(
        &mut mem,
        IMMORTAL_PARAM + layout.ffield_name,
        names.immortal,
        0,
    );
    mem.i32(IMMORTAL_PARAM + layout.fproperty_offset, 0);
    mem.i32(IMMORTAL_PARAM + layout.fproperty_element_size, 1);

    mem.u64(
        ACTOR_COMPONENT + layout.ustruct_child_properties,
        MAX_HEALTH_FIELD,
    );
    mem.u64(
        HEALTH_COMPONENT + layout.ustruct_child_properties,
        CAN_DIE_FIELD,
    );
    mem.u64(
        SET_IMMORTAL + layout.ustruct_child_properties,
        IMMORTAL_PARAM,
    );

    mem.u64(ACTOR_COMPONENT + layout.ustruct_children, ACTIVATE);
    mem.u64(HEALTH_COMPONENT + layout.ustruct_children, SET_IMMORTAL);
    mem.u32(SET_IMMORTAL + layout.ufunction_flags, 0x400);
    mem.u64(SET_IMMORTAL + layout.ufunction_func, 0x7ff6_1234_5678);
    mem.u32(ACTIVATE + layout.ufunction_flags, 0);
    mem.u64(ACTIVATE + layout.ufunction_func, 0);

    Image { mem, layout, names }
}

#[test]
fn object_array_and_identity_guards() {
    let image = synthetic_image();
    let reflection = image.reflection();

    assert_eq!(reflection.object_count(), Some(12));
    assert_eq!(reflection.object_at(6), Some(HEALTH_COMPONENT));
    assert_eq!(reflection.object_at(-1), None);
    assert_eq!(reflection.object_at(12), None);
    assert!(reflection.is_object(HEALTH_COMPONENT));
    assert!(!reflection.is_object(NOT_IN_ARRAY));
    assert!(!reflection.is_object(HEALTH_COMPONENT + 1));
    assert!(!reflection.is_object(0x0000_7fff_ffff_f000));

    let mut corrupt = image.mem.clone();
    corrupt.i32(OBJECTS + image.layout.objects_num, 17);
    let reflection = Reflection::new(&corrupt, image.layout, OBJECTS, NAMES);
    assert_eq!(reflection.object_count(), None);
    assert_eq!(reflection.object_at(0), None);
}

#[test]
fn names_support_encodings_suffixes_and_pool_bounds() {
    let image = synthetic_image();
    let reflection = image.reflection();

    assert_eq!(
        reflection.name(image.names.class, 0).as_deref(),
        Some("Class")
    );
    assert_eq!(
        reflection.name(image.names.wide, 0).as_deref(),
        Some("生命")
    );
    assert_eq!(
        reflection.name(image.names.health_component, 1).as_deref(),
        Some("HealthComponent_0")
    );
    assert_eq!(
        reflection.object_name(HEALTH_INSTANCE).as_deref(),
        Some("HealthComponent_0")
    );
    assert_eq!(reflection.name(1 << 16, 0), None);
    assert_eq!(reflection.name(image.names.hidden, 0), None);
}

#[test]
fn object_relations_type_queries_and_cycles_are_bounded() {
    let image = synthetic_image();
    let reflection = image.reflection();

    assert_eq!(reflection.class_of(HEALTH_INSTANCE), Some(HEALTH_COMPONENT));
    assert_eq!(reflection.outer_of(HEALTH_COMPONENT), Some(PACKAGE));
    assert_eq!(reflection.outer_of(PACKAGE), None);
    assert_eq!(
        reflection.path_name(HEALTH_COMPONENT).as_deref(),
        Some("/Script/Dominion.HealthComponent")
    );
    assert_eq!(reflection.super_of(HEALTH_COMPONENT), Some(ACTOR_COMPONENT));
    assert_eq!(reflection.super_of(OBJECT_CLASS), None);
    assert_eq!(
        reflection.find_type("HealthComponent"),
        Some(HEALTH_COMPONENT)
    );
    assert_eq!(reflection.find_type("StatData"), Some(STAT_DATA));
    assert!(reflection.is_a(HEALTH_INSTANCE, "ActorComponent"));
    assert!(!reflection.is_a(HEALTH_INSTANCE, "Missing"));

    let mut outer_cycle = image.mem.clone();
    outer_cycle.u64(PACKAGE + image.layout.uobject_outer, HEALTH_COMPONENT);
    let reflection = Reflection::new(&outer_cycle, image.layout, OBJECTS, NAMES);
    assert_eq!(reflection.path_name(HEALTH_COMPONENT), None);

    let mut super_cycle = image.mem.clone();
    super_cycle.u64(
        ACTOR_COMPONENT + image.layout.ustruct_super,
        HEALTH_COMPONENT,
    );
    let reflection = Reflection::new(&super_cycle, image.layout, OBJECTS, NAMES);
    assert!(!reflection.is_a(HEALTH_INSTANCE, "Missing"));
    assert_eq!(reflection.find_property(HEALTH_COMPONENT, "Missing"), None);

    let mut garbage = image.mem.clone();
    garbage.u64(HEALTH_INSTANCE + image.layout.uobject_class, 0xdead_bee0);
    let reflection = Reflection::new(&garbage, image.layout, OBJECTS, NAMES);
    assert_eq!(reflection.class_of(HEALTH_INSTANCE), None);
    assert!(!reflection.is_a(HEALTH_INSTANCE, "HealthComponent"));
    assert_eq!(reflection.path_name(0xdead_bee0), None);
}

#[test]
fn properties_walk_fields_find_inherited_values_and_stop_on_cycles() {
    let image = synthetic_image();
    let reflection = image.reflection();

    let properties = reflection.properties(HEALTH_COMPONENT);
    assert_eq!(properties.len(), 1);
    assert_eq!(properties[0].name, "bCanDie");
    assert_eq!(properties[0].type_name, "BoolProperty");
    assert_eq!(properties[0].offset, 0x140);
    assert_eq!(properties[0].size, 1);

    let inherited = reflection
        .find_property(HEALTH_COMPONENT, "MaxHealth")
        .expect("property declared on the super class");
    assert_eq!(inherited.type_name, "FloatProperty");
    assert_eq!(inherited.offset, 0x138);
    assert_eq!(inherited.size, 4);

    let mut cycle = image.mem.clone();
    cycle.u64(CAN_DIE_FIELD + image.layout.ffield_next, MAX_HEALTH_FIELD);
    cycle.u64(MAX_HEALTH_FIELD + image.layout.ffield_next, CAN_DIE_FIELD);
    cycle.u64(
        HEALTH_COMPONENT + image.layout.ustruct_child_properties,
        CAN_DIE_FIELD,
    );
    let reflection = Reflection::new(&cycle, image.layout, OBJECTS, NAMES);
    let properties = reflection.properties(HEALTH_COMPONENT);
    assert_eq!(properties.len(), 2);
    assert_eq!(properties[0].name, "bCanDie");
    assert_eq!(properties[1].name, "MaxHealth");

    let mut garbage = image.mem.clone();
    garbage.u64(
        HEALTH_COMPONENT + image.layout.ustruct_child_properties,
        0xdead_bee0,
    );
    let reflection = Reflection::new(&garbage, image.layout, OBJECTS, NAMES);
    assert!(reflection.properties(HEALTH_COMPONENT).is_empty());
}

#[test]
fn functions_resolve_native_offsets_params_and_inheritance() {
    let image = synthetic_image();
    let reflection = image.reflection();

    let functions = reflection.functions(HEALTH_COMPONENT, 0x7ff6_1200_0000);
    assert_eq!(functions.len(), 1);
    assert_eq!(functions[0].name, "SetIsImmortal");
    assert_eq!(functions[0].object, SET_IMMORTAL);
    assert_eq!(functions[0].flags, 0x400);
    assert_eq!(functions[0].native, Some(0x34_5678));
    assert_eq!(functions[0].params.len(), 1);
    assert_eq!(functions[0].params[0].name, "bImmortal");

    let underflow = reflection.functions(HEALTH_COMPONENT, u64::MAX);
    assert_eq!(underflow[0].native, None);

    let inherited = reflection
        .find_function(HEALTH_COMPONENT, "Activate")
        .expect("function declared on the super class");
    assert_eq!(inherited.object, ACTIVATE);
    assert_eq!(inherited.flags, 0);
    assert_eq!(inherited.native, None);

    let mut garbage = image.mem.clone();
    garbage.u64(
        HEALTH_COMPONENT + image.layout.ustruct_children,
        0xdead_bee0,
    );
    let reflection = Reflection::new(&garbage, image.layout, OBJECTS, NAMES);
    assert!(reflection.functions(HEALTH_COMPONENT, 0).is_empty());
}

#[test]
fn instances_and_dump_match_the_stable_json_schema() {
    let image = synthetic_image();
    let reflection = image.reflection();

    assert_eq!(
        reflection.instances_of("ActorComponent"),
        vec![HEALTH_INSTANCE]
    );

    let dumps = reflection.dump(
        &|name| matches!(name, "HealthComponent" | "StatData"),
        0x7ff6_1200_0000,
    );
    assert_eq!(dumps.len(), 2);
    let json = serde_json::to_value(&dumps).unwrap();
    let health = json
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["name"] == "HealthComponent")
        .unwrap();
    assert_eq!(health.as_object().unwrap().len(), 6);
    assert_eq!(health["kind"], "Class");
    assert_eq!(health["super"], "ActorComponent");
    assert_eq!(health["size"], "0x150");
    assert_eq!(health["props"][0]["name"], "bCanDie");
    assert_eq!(health["props"][0]["offset"], "0x140");
    assert_eq!(health["functions"][0]["name"], "SetIsImmortal");
    assert_eq!(health["functions"][0]["flags"], "0x400");
    assert_eq!(health["functions"][0]["native"], "0x345678");

    let stat_data = json
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["name"] == "StatData")
        .unwrap();
    assert_eq!(stat_data.as_object().unwrap().len(), 5);
    assert_eq!(stat_data["kind"], "ScriptStruct");
    assert_eq!(stat_data["super"], serde_json::Value::Null);
    assert_eq!(stat_data["size"], "0x20");
    assert!(stat_data.get("functions").is_none());
}
