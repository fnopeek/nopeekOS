# Mechanisch portieren statt abschreiben — Arbeitsauftrag

**Angelegt:** 2026-08-20 · Auslöser: ein Tag AX200-Debugging, der mit
`docs/plan/WIFI_AX200_LINUX_COVERAGE.md` endete — 143 von 3717
Linux-Funktionen überhaupt erwähnt, und vier von fünf Tagesfunden waren
Felder, die Linux setzt und wir nicht.

Florians Entscheidung dahinter: Zeit und Tokens gehen in die saubere
Portierung, nicht in Debug-Sessions, die mit „uns fehlt ein wesentlicher Teil"
enden. Und weiter gedacht: **AX200 ist einer von vielen Treibern.** Je
komplexer die Hardware, desto größer der Anteil, der reine Datenübernahme ist
— und desto mehr lohnt ein Werkzeug statt Handarbeit.

## Die vier Stufen

| Stufe | Was | LLM nötig? |
|---|---|---|
| **1** | Strukturen, Offsets, Register, Enums aus den Linux-Headern **erzeugen** statt abschreiben | nein |
| **2** | Nicht belegte Felder je Befehlsstruktur melden | nein |
| **3** | Zuweisungsmenge (`cmd.feld = …`) aus dem Linux-AST extrahieren und gegen unsere diffen | nein |
| **4** | Abdeckungs-Manifest je Funktion (`ported`/`partial`/`n-a`) + Hash des Upstream-Rumpfs | nein |
| — | Zustandsmaschinen, Reihenfolgen, bewusste Architektur-Abweichungen | ja |

Stufe 1–3 hätten **vier von fünf** Funden des 2026-08-20 vorher gemeldet:
`qos_flags`+`ac[]` nie geschrieben, `uapsd_acs`/`sp_length` nie gesetzt,
`agg_size` aus HT statt VHT, die falsche der drei FIFO-Tabellen.

## Was schon da ist

- `tools/linux-coverage.py` — zählt aus, welche Linux-Funktionen wir erwähnen.
  Grundlage von `WIFI_AX200_LINUX_COVERAGE.md`.
- `tools/struct-offsets.py` — **Prototyp**, 60 Zeilen Regex, liest
  `struct iwl_mac_ctx_cmd` / `iwl_tx_resp` / `iwl_ac_qos` aus `fw/api/*.h`
  und zeigt je Feld, ob wir eine Konstante darauf haben. Scheitert an
  `ac[AC_NUM+1]`, Unions und Bitfeldern — **das ist der Beweis, dass der
  Nachfolger kein besserer Regex sein darf.**

## Stufe 1 — der konkrete nächste Auftrag

**Ziel:** `tools/wasm/wifi_ax200/src/regs.rs` (1240 Zeilen handkopierte
Offsets, Register, Enums, Capability-Bits) schrumpft auf das, was wirklich
unsere Entscheidung ist. Alles, was aus Linux stammt, wird generiert.

**Weg:** `rust-bindgen` (bzw. libclang direkt) auf

    drivers/net/wireless/intel/iwlwifi/fw/api/*.h
    drivers/net/wireless/intel/iwlwifi/iwl-csr.h
    drivers/net/wireless/intel/iwlwifi/iwl-prph.h
    include/linux/ieee80211.h

mit einem kleinen Shim-Header für `__le16/__le32/__le64`, `u8/u16/u32/u64`,
`__packed`, `__aligned` — die Kernel-Typen, die sonst fehlen.

**Randbedingungen, die den Auftrag prägen:**

- Ziel ist `no_std`, `wasm32-unknown-unknown`. Generierte Bindings dürfen
  weder `std` noch `libc` ziehen (`--use-core`, `--ctypes-prefix`).
- Builds laufen `--locked`. Eine neue Build-Abhängigkeit heißt: `Cargo.lock`
  im selben Commit nachziehen (siehe CLAUDE.md, Release-Flow).
- Der Linux-Baum liegt unter `~/.cache/nopeekos/linux-src/linux-6.18.26/` und
  ist **nicht im Repo**. Die Generierung darf den Build also nicht davon
  abhängig machen — Ergebnis committen (`regs_generated.rs`) und die
  Regenerierung als separates `tools/`-Kommando anbieten, nicht als `build.rs`.

**Die Migration MUSS beweisbar neutral sein.** Nicht die Hand-Konstanten
löschen und hoffen. Reihenfolge:

1. Generieren nach `regs_generated.rs`, noch nichts benutzen.
2. Für **jede** bestehende Hand-Konstante ein
   `const _: () = assert!(HAND == GENERIERT);` — der Compiler vergleicht.
   Jede Abweichung ist ENTWEDER ein Fehler von damals ODER ein Fehler im
   Generator; beide gehören einzeln angesehen, nicht pauschal aufgelöst.
3. Erst wenn alle Zusicherungen halten: Hand-Konstanten entfernen, ein
   Bereich pro Commit.

Das ist dieselbe Disziplin wie beim byte-identischen Rendern in beak
(`memory/feedback_byte_identical_render_gate.md`): ein neutraler Umbau wird
pro Schritt EINZELN bewiesen.

**Nicht anfassen:** die Treiberlogik. Stufe 1 ist ein reiner Umbau der
Datenherkunft. Kein Verhalten darf sich ändern, keine Version des Moduls
bumpen, bis der Umbau steht.

## Danach

Stufe 2 fällt fast von selbst ab, sobald die Strukturen bekannt sind: für
jedes Feld prüfen, ob unser Code es je schreibt. Stufe 3 braucht einen
C-AST-Durchgang (libclang), ist aber ein deutlich kleineres Problem als
allgemeine C→Rust-Übersetzung — es geht nur um Zuweisungen an Befehlsfelder.

`c2rust` ist **kein** Weg: unsafe, unidiomatisch, und braucht weiter skb, RCU,
Locking und das Linux-Gerätemodell. Der Hebel liegt im Erzeugen der Daten und
im Prüfen der Logik, nicht im Übersetzen der Logik.
