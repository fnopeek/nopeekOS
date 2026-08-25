# Künstliche Grenzen — Bestandsaufnahme und Umbauplan

**Stand 2026-08-25.** Ausgelöst von Florian: *„die Grössen die du erfindest
bringen uns immermal wieder neue Probleme … wir haben RAM ohne Ende in den
heutigen PCs, die dürfen auch bisschen was brauchen."*

Dieses Papier hält fest, was gezählt wurde, was in beak schon umgebaut ist,
und was für eine eigene Session liegen bleibt. Es ersetzt kein Memory-File —
der Grundsatz steht in `memory/feedback_invented_limits.md`.

## Die drei Sorten

Eine Grenze braucht einen Grund. Es gibt genau drei, und nur die erste ist
unstrittig.

1. **Echte Invariante.** Protokoll, Spezifikation, oder was ein *feindliches*
   Gegenüber erzwingen kann. h2 `MAX_FRAME` (RFC 9113), der Headerblock-Deckel,
   das gzip-Budget gegen die Zip-Bombe. Bleibt. Die Begründung ist die
   Bedrohung, nicht der Geschmack.
2. **Erfundener Deckel auf eigenem Material.** Gehört weg oder an eine
   gemessene Grösse. Nie eine Stückzahl —
   `memory/feedback_caps_by_bytes_not_count.md`.
3. **Budget.** Gehört in einen Topf mit Verdrängung unter echtem Druck, nicht
   in fünf geratene. Der Mechanismus ist „was ist frei", nicht „was habe ich
   mir zugeteilt".

Und: schneidet etwas doch ab, **muss es das sagen**.

## Was in beak schon umgebaut ist (0.37.0)

**Der Heap wächst.** `static mut HEAP: [u8; 128 MB]` ist weg. An seiner Stelle
steht `GrowingHeap`, portiert aus talcs eigenem `WasmGrowAndExtend` mit einer
Änderung: der Wachstumsschritt verdoppelt, statt genau die fehlgeschlagene
Allokation nachzulegen. Grund: unter wasmi ist `memory.grow` **nicht** billig
(siehe unten), ein Seitenweise-Wachsen auf 60 MB kopierte zweistellige
Gigabytes.

Gemessen mit `beakbench`, Main_Page 1902x1000:

| | vorher | nachher |
|---|---|---|
| Linearer Speicher beim Start | 132 MiB | **4 MiB** |
| Spitze nach Fonts + 3 Layouts | 132 MiB (fix) | 77 MiB, gewachsen |
| `instantiate` | 6 ms | 0 ms |
| Layout warm | 460/462 ms | 472/455 ms (Rauschen) |

Die alten 128 MB waren also **kein grosszügiger Kopfraum**, sondern eine Decke
knapp über dem echten Bedarf einer normalen Seite — und wasmi alloziert sie
bei jedem Start ganz.

**Die vier Pixel-Töpfe** (`TOTAL_BUDGET`, `CSS_BUDGET`, `IMG_CACHE_BUDGET`,
`CSS_CACHE_BUDGET`) sind keine Anteile eines festen Heaps mehr. Sie heissen
jetzt, was sie sind: der Punkt, an dem wir einer Seite nicht mehr glauben.
Weit über jeder gemessenen echten Seite, und **jeder Verwurf wird geloggt** —
die nächste echte Seite, die anstösst, sagt es uns, statt still ein Bild zu
verlieren. Vier statt einer, weil ein Topf voller Icons die `<img>` nicht
aushungern darf und umgekehrt.

**`MAX_IMAGES`** ist von 64 auf 512 und heisst im Kommentar jetzt, was es ist:
eine Schranke für die Abrufwarteschlange, kein Speicherlimit.

## Der eigentliche Fix, der offen ist: virtueller Speicher für WASM

**Befund (2026-08-25, wasmi_core 1.0.9 `src/memory/buffer.rs`):** wasmi hat
ZWEI Hinterlegungen für den linearen Speicher.

```rust
match self.get_vec() {
    Some(vec) => self.grow_vec(vec, new_size),   // try_reserve + resize -> KOPIERT
    None      => self.grow_static(new_size),     // nur `len` hoch -> kopiert NIE
}
```

`grow_static` schiebt bloss die Längenmarke innerhalb einer vorher
reservierten Kapazität. Und **`wasmi::Memory::new_static(ctx, ty, &'static mut
[u8])` ist öffentlich.** Wir sind nur deshalb auf dem `Vec`-Pfad, weil er der
Standard ist.

Aus Sicht des Gastes MUSS der Speicher zusammenhängend sein — das ist
WASM-Semantik (`i32.load offset=X` adressiert ein flaches Array). Aus Sicht der
Maschine muss er es nicht: dafür gibt es Paging. Echte Engines reservieren
einen grossen VIRTUELLEN Bereich ohne physischen Speicher dahinter und legen
beim Wachsen Seiten hinein — `memory.grow` ist dann ein Seitentabellen-Eintrag,
kein Umkopieren, und der Basiszeiger wandert nie.

**Umbau:** pro WASM-Modul einen virtuellen Bereich reservieren, Frames beim
Wachsen hineinlegen, wasmi den statischen Puffer darüber geben. Danach ist die
Verdopplung in `GrowingHeap` überflüssig — präzises Wachsen wird gratis.

Der Kernel hat Paging und einen Frame-Allokator; der Kernel-Heap wächst schon
in Regionen (`kernel/src/mm/heap.rs`: 64 MB initial, 64-MB-Schritte,
`MAX_REGIONS: 32`) und ist selbst kein zusammenhängender Block. Der Mechanismus
ist da, er ist nur nicht an den WASM-Speicher gehängt.

Dasselbe Muster passt überall, wo ein grosser Puffer wächst: der Malpuffer
(7,6 MB, bei jeder Fenstergrösse neu), npkFS-Caches, der Gastspeicher der
MicroVM.

## Was noch nicht ausgezählt ist

**Nur beak wurde gezählt.** Alles hier ist ungeprüft und braucht dieselbe
Behandlung — erst zählen, dann in die drei Sorten einordnen, dann anfassen:

- `kernel/src/mm/heap.rs` — `MAX_HEAP: 2 GB`, `MAX_REGIONS: 32`,
  `GROW_CHUNK: 64 MB`. Wächst schon, aber die Decke ist geraten.
- npkFS — Pfad-, Eintrags- und Cache-Deckel.
- OTA — `MAX_MANIFEST_SIZE`, `MAX_SIG_SIZE`, die Grössenschranke
  (`memory/project_ota_size_cap.md`).
- Der Kernel-HTTP-Pfad — `MAX_REPLY_HEADERS`, `CONN_POOL_SIZE`,
  `H2_POOL_SIZE`, `MAX_BODY`, `MAX_HEADER_BLOCK`. Einiges davon ist Sorte 1
  und bleibt; das muss die Einordnung zeigen.
- Widget/Compositor — Szenen- und Ereignispuffer.

**Reihenfolge:** erst die virtuelle WASM-Hinterlegung (sie nimmt dem Rest den
Druck), dann zählen, dann einordnen. Kein Fix vor der Karte —
`memory/feedback_count_it_dont_sample_it.md`.
