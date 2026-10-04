# `tools/wasm/aml/core/src/value.rs` @ 5e0102684

## L1 · `use alloc::{rc::Rc, string::String, vec::Vec};`

```
//! AML runtime values and name paths.
```

## L6 · `pub type Seg = [u8; 4];`

```
/// A 4-byte ACPI NameSeg (trailing '_' padded).
```

## L8 · `pub type Path = Vec<Seg>;`

```
/// Absolute namespace path = chain of segments from the root.
```

## L10 · `pub type Obj = Rc<RefCell<Value>>;`

```
/// A mutable data-object cell (Name value, package element, Local/Arg slot).
```

## L24-25 · `Ref(Place),`

```
/// A reference to a writable place produced by Index / RefOf / a bare name
/// used as a Store target.
```

## L31 · `Obj(Obj),`

```
/// A plain data-object cell.
```

## L33 · `Field(Path),`

```
/// A field unit at an absolute namespace path (read/write hits its region).
```

## L35 · `BufIndex(Obj, usize),`

```
/// A byte inside a Buffer object.
```

## L37-40 · `BufField(Obj, u64, u64),`

```
/// Ein BUFFER-FELD: `CreateWordField` & Co. binden einen Namen an einen
/// Bitausschnitt EINES bestimmten Puffers. Der Puffer faehrt als `Obj`
/// mit, nicht als Kopie — sonst schriebe `INT1 = …` in einen Abzug und
/// die Ressourcenvorlage bliebe, wie sie war.
```

## L50 · `let mut n = 0u64;`

```
// First up-to-8 bytes, little-endian.
```

## L71 · `pub fn path_str(p: &Path) -> String {`

```
/// Render a path like `\_SB.PCI0.LPCB.EC0_.BRC_` for logging.
```

