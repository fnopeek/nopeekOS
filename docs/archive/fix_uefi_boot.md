# fix_uefi_boot.md — Notebook bootet wieder ✅ (2026-06-02)

> **Archiv (2026-08-11).** Gelöst und seither stabil; steht hier als
> Fehlersuche-Protokoll, nicht als offene Arbeit.

**Status: GELÖST.** Das HP-Notebook bootet wieder durch (Installer läuft).
Drei verschiedene Probleme übereinander — alle gefunden + gefixt.

## Symptom (war)
USB-Stick bootet sauber auf Intel-NUC + QEMU. Auf dem HP-Notebook: Stick
erkannt, aber schwarz → Reboot-Loop, nie ein Bild. Regression seit dem
UEFI-Refactor (2026-05-14); über den alten GRUB-Pfad (Kernel ~v0.50.6) lief
auf demselben Notebook mal eine echte Installation → Hardware ist fähig,
nur der UEFI-Entry-Pfad war kaputt.

## Die drei Ursachen (in der Reihenfolge, in der sie auftauchten)

### 1. Secure Boot (Firmware-Config)
„Selected boot image did not authenticate" — unsigniertes BOOTX64.EFI.
**Fix:** im BIOS deaktivieren (HP: Advanced → Boot/Secure-Boot-Config).

### 2. Nicht-relozierbares Image (v0.193.9)
`relocation-model=static` + `RELOCS_STRIPPED` → Firmware musste exakt auf
`ImageBase=0x10000000` laden; HP konnte nicht → Triple-Fault vor Output.
**Fix:** PIE + Self-Relocation (gnu-efi-Pattern), siehe `[[project_uefi_relocatable]]`.

### 3. Zu großes EFI-Image — **selbst verursacht an dem Tag!**
Ich hatte `./build.sh usb` so geändert, dass es die **261-MB-LibreWolf-sqfs
ins Kernel-EFI einbettet** → ~290-MB-Image. Die HP-Firmware kann ~294 MB
nicht am Stück allozieren (LoadImage scheitert) → führt den Entry nie aus →
~20s-Watchdog-Reboot. OVMF/NUC mit großzügiger Memory-Map tolerieren es.
**Fix:** `usb` baut wieder den schlanken ~26-MB-Installer (kein eingebettetes
sqfs); `usb-full` = die große Variante. Browser kommt per OTA (offline-
Auslieferung als separate Datei = Tuning für später).

### 4. PIC-Reinit-SMI-Reset — **die Kern-Ursache** (v0.193.10)
Nachdem das Image lud, crashte der Kernel sofort. Per **farbcodierter
On-Screen-Diagnostik** (das Notebook hat keine Serial!) bis auf die Zeile
eingegrenzt: **`interrupts::init()` → `pic_remap()` → `outb(0x20, 0x11)`
(ICW1)**. Die HP/Insyde-Firmware **virtualisiert den Legacy-8259-PIC per SMM
und trappt Command-Port-Writes als SMI**, der post-ExitBootServices die
Maschine resettet.
**Fix:** Legacy-PIC **nicht neu-initialisieren** — UEFI übergibt im APIC-
Modus. Nur maskieren (Daten-Ports), Ticks vom Local-APIC-Timer
(`init_apic_timer`, nutzte der Kernel auf UEFI eh). `io_wait`s Port-0x80-
Write (auch SMI-getrappt) gleich mit entfernt.

### 5. Folgefehler: interne PS/2-Tastatur tot
PIC maskiert → IRQ1 feuert nie → die interne PS/2-Tastatur lieferte nichts
(USB/xhci ging). Installer-Prompt `[y/N]` nicht bedienbar.
**Fix:** **PS/2-Polling-Fallback** in `keyboard::read_key`/`has_key` — den
i8042 direkt abfragen (latcht Scancodes auch ohne IRQ), Aux/Maus-Bytes
übersprungen. USB-Pfad unverändert.

## Diagnose-Technik (für nächstes Mal, serial-loses Gerät)
- **Farbcodierte Framebuffer-Stufen** + **ConOut-Text vor ExitBootServices**
  → zeigt auf dem Schirm, wie weit der Boot kommt. Letzte stehende Farbe =
  Crash-Stelle. So ohne Serial die exakte Zeile gefunden.
- **Triple-Fault-Timing-Test** (Null-IDT + ud2) für „läuft unser Code
  überhaupt?" — sofortiger Reset = ja, ~20s = Firmware-Watchdog (Code lief nie).
  ⚠️ Lektion: ein hängendes `hlt` sieht auf echter Firmware wie ein Reboot aus
  (UEFI-Watchdog ~20s). Hang-vs-Reboot ist als Signal mehrdeutig.

## Offen (Tuning, später)
- Offline-Browser auf dem USB-Stick **ohne** das EFI aufzublähen (sqfs als
  separate Datei auf der ESP, Installer liest sie → klein bootbar + offline).
- PS/2 evtl. sauber über IOAPIC routen statt pollen (optional).
- HP-Notebook-Installation + Post-Install-Reboot end-to-end bestätigen.
