# nopeekOS

**Ein AI-natives Betriebssystem. Von Grund auf neu gedacht.**

Kein Unix-Klon. Kein POSIX. Kein Erbe aus den 70ern.
Ein System, in dem AI der primäre Operator ist – und der Mensch der Dirigent.

---

## Die Grundidee

Warum sieht ein Betriebssystem aus wie es aussieht? Weil 1969 Menschen
jeden Befehl manuell tippen mussten. Weil 1984 eine Desktop-Metapher
nötig war. Weil 2024 immer noch Menschen Prozesse managen, Treiber
installieren, und `chmod 755` eintippen.

nopeekOS dreht die Frage um: **Was bleibt übrig, wenn man fünfzig Jahre
an Annahmen entfernt?**

Die Antwort:

- Ein **Capability Vault** statt Permissions
- Eine **WASM Sandbox** statt Prozesse
- Ein **Intent Loop** statt eine Shell
- Ein **Content Store** statt ein Dateisystem
- Ein **Human View** statt ein Desktop

Alles andere wird zur Laufzeit generiert.

---

## Architektur

```
┌─────────────────────────────────────────────────────────────┐
│  Human View                                                 │
│  Minimaler, adaptiver Canvas. Serial → Framebuffer → UI.   │
├─────────────────────────────────────────────────────────────┤
│  Intent Loop                                                │
│  Nimmt Absichten entgegen, nicht Befehle.                   │
│  Phase 1-4: Pattern Matching    Phase 5+: AI Resolution    │
├─────────────────────────────────────────────────────────────┤
│  WASM Runtime (wasmi → Cranelift JIT)                       │
│  Jede Ausführung ist ein sandboxed WASM-Modul.             │
│  Capability-gated Host Functions statt Syscalls.            │
├─────────────────────────────────────────────────────────────┤
│  nopeekOS Kernel (Rust, no_std)                                │
│  Memory Manager │ Capability Vault │ Audit Log │ Scheduler  │
├─────────────────────────────────────────────────────────────┤
│  Hardware (x86_64, später aarch64)                          │
│  UEFI/Multiboot2 → Long Mode → Rust                        │
└─────────────────────────────────────────────────────────────┘
```

---

## Kernprinzipien

### Capabilities statt Permissions

Kein `chmod`, keine ACLs, kein root, kein sudo.
Jede Ressource wird durch ein kryptographisches Token repräsentiert.
Ein WASM-Modul kann nur das tun, wofür es ein Token hat.
Tokens können delegiert (mit Einschränkung) und revoked werden.
Alles ist auditierbar.

```rust
Capability {
    id:         u128,
    resource:   ResourceKind,   // Memory, IO, Net, Store, ...
    rights:     Rights,         // Read, Write, Execute, Delegate
    scope:      Scope,          // Zeitlich + umfangmässig limitiert
    provenance: ProvenanceChain // Wer hat es erstellt/delegiert
}
```

**Security-Modell: Deny by Default.** Ohne Token passiert nichts.
Kein "ambient authority". Kein implizites Vertrauen.

### Intents statt Commands

Statt `nmap -sV 10.0.0.0/24` sagst du: *"Scan das Netzwerk 10.0.0.0/24"*.
Der Intent Resolver entscheidet, welches WASM-Modul das erfüllt,
welche Capabilities nötig sind, und ob der Intent sicher ist.

### Content-Addressable Store statt Filesystem

Keine Pfade. Keine Hierarchie. Keine DLL-Hell.
Jedes Objekt ist durch seinen BLAKE3-Hash adressiert.
Suche läuft über semantische Tags, nicht über `/usr/bin/`.

### WASM als universelle Execution Unit

Jede Ausführung ist ein WASM-Modul in einer Sandbox:
- Kein direkter Hardware-Zugang
- Nur über Host Functions (capability-gated)
- Module sind ephemeral – nach Ausführung garbage-collected
- Häufig genutzte Module werden cached (content-addressed)
- Trust-Stufenmodell: Interpret → Verify → JIT → Native

---

## Phasen

### Phase 1 – Bare Metal Boot ← AKTUELL

Das absolute Minimum: Von Strom zu Rust.

- [x] Multiboot2 Header
- [x] 32→64 bit Long Mode Transition (Page Tables, PAE, GDT)
- [x] Rust `no_std` Kernel Entry Point
- [x] Serial Console Driver (COM1, 115200 baud)
- [x] VGA Boot Banner (visueller Indikator)
- [x] Kernel Print Macros (`kprintln!`)
- [ ] Physical Memory Manager (Buddy Allocator)
- [ ] Interrupt Handling (IDT, PIC/APIC)
- [ ] Paging (Higher-Half Kernel Mapping)
- [ ] Basic Heap Allocator (für `alloc` Crate)

**Ziel:** nopeekOS bootet in QEMU/VirtualBox, zeigt den Banner,
akzeptiert Input über Serial Console.

**Erfolgskriterium:** `npk> echo hello` gibt `hello` zurück.

### Phase 2 – Capability System

Das Sicherheitsfundament, von Anfang an.

- [x] Capability Vault (in-memory Token Registry)
- [x] Token Creation, Delegation, Revocation
- [x] Transitive Revocation (Kinder werden mit-revoked)
- [x] Rechte-Monotonie (delegierte Rechte ≤ Parent-Rechte)
- [ ] Temporal Scoping (Capabilities mit Ablaufzeit)
- [ ] Audit Log (unveränderlicher Ring Buffer)
- [ ] Capability-basierte IPC zwischen Modulen

**Ziel:** Jede Operation im System erfordert ein gültiges Token.

**Erfolgskriterium:** Ein Intent ohne passende Capability wird
sauber abgelehnt mit Audit-Eintrag.

### Phase 3 – WASM Runtime

Die universelle Ausführungsumgebung.

- [ ] wasmi Integration (no_std + alloc)
- [ ] Host Function Binding (capability-gated)
- [ ] WASM-Modul Loader (aus Content Store)
- [ ] Sandbox Enforcement Tests
- [ ] Basis-WASI Support (fd_write, fd_read, clock_time_get)
- [ ] Modul-Lifecycle (Load → Execute → Cleanup)

**Ziel:** Beliebiger Code läuft isoliert als WASM-Modul.

**Erfolgskriterium:** Ein in Rust geschriebenes WASM-Modul
wird geladen, führt eine Berechnung durch, gibt das Ergebnis
über eine Host Function zurück – alles capability-gated.

### Phase 4 – Intent Loop + WASM Integration

Der Intent Loop wird zum echten Dispatcher.

- [ ] Intent → WASM Module Mapping (statische Lookup Table)
- [ ] Capability Scoping pro Intent
- [ ] Result Formatting und Display
- [ ] Error Handling (Modul crasht → System läuft weiter)
- [ ] Intent History (Content Store)
- [ ] Basis-Intents als WASM-Module (statt hardcoded)

**Ziel:** Intents werden durch WASM-Module erfüllt, nicht
durch hardcoded Rust-Funktionen.

**Erfolgskriterium:** `npk> hash "hello world"` ruft ein
WASM-Modul auf, das den BLAKE3-Hash berechnet und zurückgibt.

### Phase 5 – Content-Addressable Store

Persistenz ohne Dateisystem.

- [ ] BLAKE3-basierter Content Store
- [ ] In-Memory Store mit Persistence auf virtio-blk
- [ ] Semantisches Tagging
- [ ] Garbage Collection für nicht-referenzierte Objekte
- [ ] WASM-Module im Store cachen
- [ ] Deduplizierung

**Ziel:** Alles wird content-adressiert gespeichert,
nichts geht verloren, nichts ist doppelt.

### Phase 6 – Netzwerk + WASI-Erweiterung

Die Aussenwelt anbinden.

- [ ] virtio-net Treiber (als capability-gated Modul)
- [ ] TCP/IP Stack (smoltcp als WASM oder native)
- [ ] Erweiterte WASI-Unterstützung
- [ ] DNS Resolution
- [ ] TLS (rustls)
- [ ] HTTP Client als WASM-Modul

**Ziel:** nopeekOS kann mit dem Netzwerk kommunizieren.

**Erfolgskriterium:** `npk> fetch https://example.com`
gibt den Response Body zurück.

### Phase 7 – AI Integration

Hier wird nopeekOS zu dem, was es sein soll.

- [ ] External AI Service via virtio-net
- [ ] Intent Resolution durch LLM
- [ ] Runtime WASM-Generierung (AI schreibt Module)
- [ ] Semantic Search im Content Store (Embeddings)
- [ ] neurofabric Integration (Micro-LLM Fabric)

**Ziel:** Der Mensch drückt eine Absicht aus,
die AI generiert den Code, das System führt ihn aus.

### Phase 8 – Human View

Das adaptive Interface.

- [ ] Framebuffer-Treiber (VESA/GOP)
- [ ] Minimaler Text-Renderer (Bitmap Font)
- [ ] Deklaratives UI-System
- [ ] AI-generierte Views
- [ ] Input-Handling (Keyboard, Mouse via virtio-input)

**Ziel:** Der Mensch sieht nicht das System,
sondern eine Projektion des System-Zustands
die für ihn optimiert ist.

---

## Technische Entscheide

| Bereich            | Wahl                | Begründung                                     |
|--------------------|---------------------|------------------------------------------------|
| Kernel-Sprache     | Rust (no_std)       | Memory Safety ohne GC, eliminiert 70% CVE-Klassen |
| Boot Protocol      | Multiboot2          | QEMU/GRUB/VirtualBox Support, simpler als UEFI |
| Target             | x86_64              | QEMU + VirtualBox default, später aarch64      |
| WASM Runtime       | wasmi → Cranelift   | no_std-kompatibel, später JIT                  |
| Content Hashing    | BLAKE3              | Schnell, sicher, Rust-nativ                    |
| Debugging          | QEMU GDB Stub       | Step-Through im Kernel                         |
| Demo/Testing       | VirtualBox           | GUI, Snapshots, bereits installiert            |

---

## App-Kompatibilität

nopeekOS ist kein POSIX-System. Existierende Software läuft nicht direkt.
Es gibt mehrere Strategien um das zu adressieren:

| Strategie                         | Was es bringt                                     |
|-----------------------------------|---------------------------------------------------|
| **WASI-Kompilation**              | C/Rust/Go → WASM. SQLite, CLI-Tools etc.          |
| **POSIX-Shim (WASM-Modul)**      | Übersetzt POSIX-Syscalls in Host Functions         |
| **Micro-VM (Firecracker-style)**  | Legacy Linux-Apps in isolierter VM                 |
| **Cloud Gaming (GeForce Now)**    | Streaming statt lokaler GPU-Stack                  |
| **Intent-basierter Ersatz**       | "Bearbeite Dokument" statt LibreOffice installieren|

Langfristiges Ziel: **Du brauchst die App nicht, du brauchst das Ergebnis.**

---

## Security-Architektur

Prinzipien die ab Zeile 1 gelten:

1. **Deny by Default** – Ohne Capability-Token passiert nichts
2. **Least Privilege** – Jedes WASM-Modul bekommt nur was es braucht
3. **Temporal Scoping** – Capabilities laufen ab
4. **Audit Everything** – Jede Token-Operation wird geloggt
5. **Formal Boundaries** – WASM-Sandbox ist die Trust Boundary
6. **No Ambient Authority** – Kein root, kein sudo, keine Elevation

Angriffsfläche: ~50'000–100'000 Zeilen Rust statt 30M+ Zeilen C (Linux)
oder 50M+ (Windows). Faktor 300-600x weniger Code in der Trust Boundary.

---

## Projektstruktur

```
nopeekOS/
├── README.md                    # Diese Datei
├── CLAUDE.md                    # AI-Entwicklungsguide
├── Cargo.toml                   # Workspace
├── build.sh                     # Build + QEMU/VirtualBox Launch
├── rust-toolchain.toml          # Nightly + Komponenten
├── x86_64-nopeekos.json         # Custom Bare-Metal Target
├── .cargo/
│   └── config.toml              # Build-Konfiguration
├── .gitignore
└── kernel/
    ├── Cargo.toml
    ├── linker.ld                # Memory Layout
    └── src/
        ├── boot.s               # Multiboot2 + Long Mode Transition
        ├── main.rs              # Kernel Entry Point
        ├── serial.rs            # Serial Console (Human Interface)
        ├── capability.rs        # Capability Vault
        ├── intent.rs            # Intent Loop
        ├── store.rs             # Content Store (Placeholder)
        └── vga.rs               # VGA Boot Banner
```

---

## Setup

### Voraussetzungen

```bash
# Rust Nightly (wird automatisch via rust-toolchain.toml gesetzt)
rustup toolchain install nightly
rustup component add rust-src llvm-tools-preview --toolchain nightly

# Build-Tools
sudo apt install grub-pc-bin xorriso mtools

# VM (mindestens eines)
sudo apt install qemu-system-x86     # Für Entwicklung + Debugging
# VirtualBox: bereits installiert     # Für Demo + visuelles Testing
```

### Build

```bash
chmod +x build.sh
./build.sh build        # Kompiliert Kernel + erstellt ISO
```

### Run

```bash
# QEMU (Entwicklung) – Serial Console auf stdio
./build.sh qemu

# QEMU mit GDB-Stub (Debugging) – wartet auf GDB-Verbindung
./build.sh debug
# In zweitem Terminal:
# gdb target/x86_64-unknown-none/debug/nopeekos-kernel -ex 'target remote :1234'

# VirtualBox (Demo) – erstellt/aktualisiert VM automatisch
./build.sh vbox

# VirtualBox entfernen
./build.sh vbox-clean
```

### Erster Boot

Nach erfolgreichem Build und `./build.sh qemu` solltest du sehen:

```
                                __   ____  _____
   ____  ____  ____  ___  ___  / /__/ __ \/ ___/
  / __ \/ __ \/ __ \/ _ \/ _ \/ //_/ / / /\__ \
 / / / / /_/ / /_/ /  __/  __/ ,< / /_/ /___/ /
/_/ /_/\____/ .___/\___/\___/_/|_|\____//____/
           /_/

[npk] AI-native Operating System v0.1.0
[npk] Booting...

[npk] Multiboot2: verified
[npk] Initializing Capability Vault...
[npk] Vault online. Root capability issued.
[npk] Initializing Content Store...
[npk] Store online.
[npk] Starting Intent Loop...

[npk] ====================================
[npk]  System ready. Express your intent.
[npk] ====================================

npk>
```

Verfügbare Intents (Phase 1):

```
npk> status          # System-Übersicht
npk> caps            # Capability Vault Info
npk> echo hello      # I/O-Test
npk> think <frage>   # AI-Platzhalter
npk> about           # Über nopeekOS
npk> philosophy      # Design-Philosophie
npk> halt            # System herunterfahren
```

---

## Was nopeekOS NICHT ist

- **Kein Linux-Klon** – kein systemd, kein ext4, kein procfs
- **Kein POSIX-System** – kein fork(), kein exec(), keine Pipes
- **Kein Unikernel** – nicht single-purpose, sondern multi-intent
- **Kein Container-Runtime** – WASM-Module sind leichter als Container
- **Kein Desktop-OS (noch nicht)** – erst Fundament, dann Interface
- **Kein akademisches Experiment** – jede Phase produziert lauffähigen Code

---

## Langfristige Vision

```
Heute:     Mensch installiert App → konfiguriert → bedient → debuggt
Morgen:    Mensch äussert Absicht → System generiert → führt aus → liefert
```

nopeekOS ist der Versuch, "morgen" zu bauen.
Ohne Kompromisse an die Vergangenheit.
Aus Luzern.

---

## Lizenz

TBD – Evaluierung zwischen MIT, Apache 2.0, und proprietär.

## Author

nopeek – [nopeek.ch](https://nopeek.ch)
