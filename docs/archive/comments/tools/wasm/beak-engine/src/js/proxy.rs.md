# `tools/wasm/beak-engine/src/js/proxy.rs` @ 5e0102684

## L1-16 · `use alloc::rc::Rc;`

```
//! `Proxy` und `Reflect`s Gegenstueck dazu.
//!
//! Ein Stellvertreter ist ein Objekt mit einer eigenen Art
//! (`ObjKind::Proxy`); jede Grundoperation des Objektmodells fragt ihn
//! zuerst. Die Haken sitzen deshalb nicht hier, sondern dort, wo die
//! Operation ohnehin steht — `Interp::get`, `set`, `has_property`,
//! `delete_key`, `own_keys_of`, `get_own_desc`, `define_own`,
//! `proto_of`/`set_proto_of`, `call` und `construct`. Diese Datei baut den
//! Konstruktor und die gemeinsame Hilfe, die eine Falle holt.
//!
//! **Benannt statt verschwiegen: die INVARIANTEN sind nicht geprueft.** Die
//! Spezifikation verlangt nach jedem Fallenaufruf einen Abgleich mit dem
//! Ziel (eine nicht konfigurierbare Eigenschaft darf nicht verschwinden, ein
//! nicht erweiterbares Ziel keine neuen Schluessel melden, …). Wir rufen die
//! Falle und glauben ihr. Das ist fuer eine Seite folgenlos — sie belaegt
//! sich selbst —, aber es ist eine Luecke gegen die Spezifikation.
```

## L25 · `pub type ProxyCell = Rc<core::cell::RefCell<Option<(Gc, Gc)>>>;`

```
/// Ziel und Behandler eines Stellvertreters, oder `None` nach dem Widerruf.
```

## L32 · `pub fn is_proxy(v: &Value) -> bool {`

```
/// Ist dieser Wert ein Stellvertreter?
```

## L37-38 · `pub fn trap(i: &mut Interp, o: &Gc, name: &str) -> C<Option<(Value, Value, Value)>> {`

```
/// Ziel und Falle holen. `Ok(None)` heisst: keine Falle, die Operation geht
/// unveraendert ans Ziel.
```

## L48-53 · `Ok(Some((f, hv, Value::Obj(t))))`

```
// **Der BEHANDLER ist der Empfaenger der Falle** (`Call(trap, handler,
// args)`, ES 10.5.x — in jeder einzelnen). Vorher lief jede Falle mit
// `this === undefined`: ein Behandler, der als KLASSE geschrieben ist,
// fand seine eigenen Felder nicht. Vues Reaktivitaet ist genau so
// gebaut (`class { constructor(){ this._isReadonly = … } get(t,k){ …
// this._isReadonly … } }`) — jedes `reactive()` starb daran.
```

## L57 · `pub fn target(i: &mut Interp, o: &Gc) -> C<Gc> {`

```
/// Das Ziel eines Stellvertreters — fuer die Faelle ohne Falle.
```

## L66-68 · `pub fn key_value(key: &str) -> Value {`

```
/// Einen Eigenschaftsnamen zurueck in einen JS-Wert — eine Falle bekommt den
/// Schluessel, wie ein Skript ihn geschrieben haette, also ein Symbol als
/// Symbol.
```

## L82-84 · `let g = new_kind(None, ObjKind::Proxy(cell.clone()));`

```
// Der Prototyp des Stellvertreters wird nie gelaufen — jeder Zugriff geht
// ueber die Fallen —, aber `new Proxy(f, {})` muss aufrufbar bleiben, und
// dafuer schaut `is_callable` auf das ZIEL.
```

## L101-103 · `let f = native(Some(i.realm.function_proto.clone()), |i, t, _| {`

```
// Der Widerruf haengt am Stellvertreter selbst: die Funktion findet
// ihn ueber ein NUL-praefigiertes Feld, weil ein Zeiger keinen
// Abschluss nimmt.
```

## L116 · `let bound = new_kind(Some(i.realm.function_proto.clone()), ObjKind::Bound {`

```
// `revoke` ruft sich selbst als `this` — dafuer wird es gebunden.
```

## L126-127 · `let _ = String::new();`

```
// `Reflect.ownKeys` und die uebrigen Reflect-Funktionen laufen ueber
// dieselben Grundoperationen wie der Rest — sie brauchen hier nichts.
```

## L131 · `pub const REVOKE_TARGET: &str = "\0!revoke";`

```
/// Wo `revoke` seinen Stellvertreter findet.
```

