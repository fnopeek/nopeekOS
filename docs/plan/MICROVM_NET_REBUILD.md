# Neubau der Schnittstelle Host-Netz ↔ microVM

*Auftragspapier, 2026-08-22. Entscheidungen darin sind gefallen, nicht offen.*

## Auftrag

Bau die Schnittstelle zwischen Host-Netz und microVM neu. Ziel ist **ein**
Datenpfad, den QEMU und Blech gleichermassen ausführen — nicht ein reparierter
von zweien.

Nicht debuggen. Der Fehler wird nicht gesucht, er wird wegkonstruiert.

## Warum nicht weitersuchen

`full_backend = FULL_RX_BACKEND && matches!(current_vendor(), Vendor::Amd)`
(`kernel/src/microvm/cpu/mod.rs:1299`). Auf AMD läuft `service_full` →
`l3_rewrite_inbound` (`nat.rs:1085`). Auf Intel läuft `pump`/`pump_fast` →
`l3_inbound` (`nat.rs:970`) → `INBOUND_Q` → GRO → `drain_inbound`.

Der Entwicklungsrechner ist AMD. **NUC und HP-Notebook sind beide Intel.** Die
Testmaschine und die Zielmaschine führen seit Monaten verschiedene Programme aus.
Fünf echte Bugs wurden auf dieser Jagd gefunden und gefixt (0.293–0.300); keiner
war der Fehler, und keiner konnte es sein.

Dazu: die Datenebene kennt `netdev` nicht, sie kennt die QEMU-virtio-NIC. Acht
Stellen fragen hart `crate::drivers::virtio_net::rx_irq_vector()` bzw.
`rx_used_idx()` — `net_dataplane.rs:52/227/287/444`, `net_backend.rs:49`,
`nat.rs:1566/1638`, `cpu/mod.rs:1248`. Auf Blech gibt es keine virtio-NIC, beide
liefern 0: `has_work()` wird zu `0 != 0`, der Worker parkt nie auf einem IRQ, und
`route_to_current(0)` routet Vektor null. Das ist keine Regression — das ist auf
dieser Hardware nie gelaufen.

Und drittens: **wo ein Frame eintritt, hängt an der Karte.** Kabel/virtio:
`netdev::recv` hinter dem POLLING-Guard, den `service_full` sich nimmt. AX200:
der WASM-Treiber liefert über `net::wasm_deliver_rx` direkt in
`eth::handle_frame` — dorthin sieht `service_full` gar nicht.

## Was schon entschieden ist

**L3-NAT bleibt.** Der Gast behält `10.99.0.2/24` und das Masquerade. Grund: eine
802.11-Station darf nur Frames mit der eigenen Adresse senden; eine Bridge über
WLAN braucht 4-Adress-Rahmen (WDS) und die Mitwirkung des AP. Der AX200 ist der
Hauptlink des Notebooks. Bridge am Kabel und NAT auf Funk wären wieder zwei
Pfade — genau die Krankheit. NAT ist nicht der Fehler; dass es die Übersetzung
zweimal gibt, ist der Fehler.

**Tap-Modell.** Statt „der Worker leert die Host-NIC" gilt „der Worker leert
seinen eigenen Ring". Damit verschwindet die NIC-Abhängigkeit vollständig.

## Zielarchitektur

```
        beliebige Host-NIC  (virtio · intel · rtl8153 · AX200/WASM)
                    │
             eth::handle_frame        ← einziger Eintritt, für alle Karten gleich
                    │
             ipv4::handle_ipv4
                    │
        nat::tap_inbound(ip)          ← EINE Übersetzung, ein Abnahmetest:
                    │                    „passt es auf ein Mapping?"
             TAP  (SPSC-Ring)         ← das tun.c-Äquivalent, beschränkt,
                    │                    Verlust bei voll = gezählter Drop
        net_dataplane Worker-Fiber    ← parkt auf SEINER Türklingel, nie auf
                    │                    dem MSI-X einer fremden Karte
          VirtioNet::inject_rx
                    │
        raise_irq() + kick_vcpu(0)    ← vendorneutral, SVM *und* VMX
```

Rückrichtung symmetrisch: Gast klingelt (`note_tx_kick`) → Worker →
`nat::tap_outbound` → `netdev::send`. Der Worker ist alleiniger Konsument beider
Gast-Ringe; kein vCPU-Exit fasst sie an.

## Vorlage ist Linux, nicht unsere Erfindung

Hole die Dateien, lies sie, portiere die Mechanik. `feedback_linux_strict` und
`feedback_read_the_vendor_source` gelten.

| Was | Linux-Quelle |
| --- | --- |
| Der Ring, sein Deckel, der Drop bei voll | `drivers/net/tun.c` — `tun_net_xmit`, `tun_do_read` |
| Worker-Schleife, Gegendruck, Batch | `drivers/vhost/net.c` — `handle_rx`, `handle_tx` |
| IRQ nur bei `used_event`-Schwelle | `drivers/vhost/vhost.c` — `vhost_add_used_and_signal`, `vhost_notify` |
| IRQ + vCPU-Wake in einem | `virt/kvm/eventfd.c` — irqfd |
| Masquerade-Zustand, Zeitschranken | `net/netfilter/nf_nat_core.c`, `nf_conntrack_proto_{tcp,udp}.c` |

Der heutige Kopf von `net_dataplane.rs` beruft sich bereits auf vhost — der Code
darunter tut aber etwas anderes: er ist der NIC-Drainer. Genau dort wurde die
Portierung abgebrochen. Dort weitermachen.

## Was gelöscht wird

Ersatzlos. Nicht auskommentiert, nicht hinter ein Flag.

- `nat::l3_inbound` (`nat.rs:970`) — die zweite Übersetzung
- `INBOUND_Q`, `INBOUND_MAX`, `drain_inbound`, `drain_to_guest`,
  `rx_producer_drain`, `wake_consumer`, `KICK_GAP_US`
- `nat::pump`, `nat::pump_fast` und ihre Aufrufe in `svm/enable.rs`
  (1608, 2020, 2275, 2308) und `vmx/enable.rs` (1848, 2168, 2202)
- GRO in Gänze: `gro_offer`, `gro_finalize`, `gro_flush_*`, `gro_alloc_slot`,
  `GRO_COALESCE`, `GRO_SLOTS`, `GRO_MAX_SEGS`, `GRO_START_MIN`. **Der Gewinn war
  echt und ist belegt** (`project_microvm_rx_gro`) — er kommt später als reine
  Funktion beim *Entnehmen* aus dem Tap zurück, gemessen, nicht auf Verdacht. Bis
  dahin ist er raus, damit der neue Pfad einen ehrlichen Nullpunkt hat.
- `FULL_RX_BACKEND` samt der `&& Amd`-Bedingung, `WARM_THROUGH_TRANSFER`,
  `NETSTAT_DEBUG`, `NETSTAT_WINDOW`, `TX_OFFLOAD_ENABLED`
- `net::try_acquire_drain` / `net::release_drain` und der `skip_nic_drain`-Fall in
  `net::poll` (`net/mod.rs:111`). Core 0 darf die NIC wieder leeren, während eine
  VM läuft — was es hineinlegt, hat jetzt einen Abnehmer.
- `RX_STAGE` in `net_dataplane.rs`. Es gibt genau einen Ring, und das ist der Tap.

## Was neu entsteht

**1. Ein RX-Wecksignal in `netdev`, kartenneutral.** Heute fragt die Datenebene
die virtio-NIC direkt. Stattdessen `netdev::rx_wake_vector() -> Option<u8>`
(MSI-X-Vektor zum Parken, `None` = gepollt) und `netdev::rx_seq() -> u64`
(monoton: „so viele Frames hat der Treiber bereitgestellt"). Jede Karte
beantwortet das selbst; `Active::Wasm` zählt in `wasm_deliver_rx` /
`wasm_nic_submit_rx` mit.

Abnahme dieses Schritts, prüfbar: `grep -rn "drivers::virtio_net"
kernel/src/microvm/` muss leer sein.

**2. Der Tap.** SPSC-Ring, feste Kapazität, Frame-Puffer aus dem vorhandenen
`FRAME_POOL` — kein Heap im Datenpfad, die Begründung in `nat.rs:274` gilt
weiter. `tap_push` gibt bei Voll `false` und zählt; das ist `tun_net_xmit`s
`tx_dropped`, kein stiller Verlust. Auf der Flanke leer→belegt: Türklingel setzen
und `kick_host_core(worker)`.

**3. `nat::tap_inbound(ip: &[u8]) -> bool`** — eine Übersetzung, aus
`l3_rewrite_inbound` (der reinen) hervorgegangen, gerufen aus `ipv4::handle_ipv4`.
Sie ist der einzige Abnahmetest, und zwar **vor** dem heutigen „ist die
Zieladresse zufällig unsere?"-Filter. Siehe Verdächtiger 1.

**4. Der Fold für VMX.** `net_backend::take_irq()` wird heute nur an
`svm/enable.rs:1647` konsumiert, `kick_bsp_net_irq` existiert nur unter `svm/`.
Beides nach `vmx/` spiegeln (`pending_irqs |= 1 << 10` im Exit-Handler,
`VCPU_HOST_CORE`-Kick). `kick_bsp_net_irq` gehört danach nach `cpu/mod.rs`, nicht
in einen der beiden Vendor-Zweige. Hier endet die Vendor-Verzweigung im Netzpfad.

## Drei Verdächtige, die der Umbau qua Bauart erledigt

Nicht gemessen — benannt, damit klar ist, wovon der Umbau befreit.

**1. `ipv4::handle_ipv4:55` filtert vor dem Masquerade-Test.** Ein Paket wird
`l3_inbound` nur angeboten, wenn `dst_ip == arp::our_ip()` **jetzt gerade**.
`L3Map` (`nat.rs:255`) speichert die Host-IP nicht. Ändert sie sich mitten in der
Sitzung — DHCP-Erneuerung, ein Carrier-Blinzeln, das `net/mod.rs:325`
`reconfigure()` auslöst — sind alle laufenden Mappings still verwaist: ausgehend
gehen sie mit der neuen Adresse unter einem Port hinaus, den der Server nicht
kennt; eingehend fallen die Antworten an Zeile 55, bevor irgendetwas sie sieht.
Das ist wörtlich „2–3 s Netz, dann kommt nichts mehr herein", und es ist die
Fehlerklasse aus `feedback_a_constant_right_at_bringup_is_wrong_later`. Unter
QEMU/slirp gibt es innerhalb einer Sitzung keine Erneuerung.
→ Im Tap-Modell ist die Host-IP Teil des Mappings, und der Abnahmetest lautet
„passt es auf ein Mapping", nicht „ist die Adresse die, die wir zufällig gerade
haben".

**2. `INBOUND_Q` ohne Abnehmer.** Auf dem Voll-Pfad kehren `pump` und `pump_fast`
sofort zurück (`nat.rs:1705`, `1728`), `l3_inbound` füllt aber weiter. Gewinnt der
AX200-Treiber den POLLING-Guard, landet das Gastpaket dort und bleibt liegen; ab
1024 gibt `l3_inbound` `true` („verbraucht") zurück und verwirft ab da jedes.
→ Es gibt nur noch einen Ring und einen Abnehmer.

**3. `l3_map_out` ist O(n²).** Für jeden Kandidatenport ein Durchlauf über alle
1024 Einträge (`nat.rs:628`). Bei vollem Tisch ~10⁶ Vergleiche pro neuem Fluss,
unter der Sperre. Der Browser öffnet 65 UDP-Flüsse pro Seite.
→ Freiliste oder Bitmap, wie `nf_nat_l4proto_unique_tuple`.

## Reihenfolge, jeder Schritt mit einem Tor

Ein Umbau pro Schritt, jeder einzeln nachgewiesen — sinngemäss
`feedback_byte_identical_render_gate`. Nach jedem Schritt ist der Baum lauffähig.

> **Florian startet die Läufe, nicht du.** `./build.sh qemu`, `debug`, `vbox`,
> `usb`, jede Hardware-Runde: seine Sache. Kompilieren (`./build.sh build`) und
> der Release-Flow bleiben deine. Am Ende jeder Stufe drei Zeilen — Befehl,
> erwartetes Ergebnis, Abbruchkriterium — dann **anhalten und warten**. Er ist das
> Gate; ein Tor, das du selbst durchschreitest, ist keins.

1. **RX-Wecksignal in `netdev`.** Verhaltensneutral.
   *Tor:* `grep -rn "drivers::virtio_net" kernel/src/microvm/` leer; QEMU-Browser
   lädt eine Seite wie vorher.
2. **VMX-Fold + Kick.** Noch ungenutzt.
   *Tor:* baut, QEMU unverändert.
3. **Tap + `tap_inbound`, unter `service_full` gehängt, Vendor-Gate fällt.**
   *Tor:* QEMU lädt auf **beiden** Vendoren — der AMD-Lauf ist ab hier nicht mehr
   privilegiert.
4. **Inline-Pfad löschen** (Liste oben).
   *Tor:* QEMU lädt; `grep -c` auf die gelöschten Namen ist 0.
5. **Blech.** Notebook mit Kabel, dann mit AX200. Erst hier zählt das Ergebnis.
6. **Erst danach:** GRO als Entnahme-Funktion zurück, gemessen —
   `feedback_the_fix_has_a_price_too`.

## Woran man sieht, dass es läuft

Ein Pfad zu haben nützt nichts, wenn niemand belegen kann, dass beide Maschinen
ihn nehmen. `feedback_the_fast_path_must_say_it_ran`,
`feedback_log_the_version_in_the_trace`:

- `netstat` bekommt eine Zeile mit **Version, Vendor, aktiver Karte und
  Pfad-Kennung**. Auf dem NUC, dem Notebook und unter QEMU muss die Pfad-Kennung
  dieselbe sein. Das ist die eigentliche Abnahme des ganzen Umbaus.
- Drei getrennte Zähler, nie einer: Tap voll (Gegendruck, gesund),
  Masquerade-Tisch voll (kein neuer Fluss möglich), Egress abgelehnt (nie auf dem
  Draht). Die Begründung steht schon in `nat.rs:116` und gilt weiter.
- Fortschritt messen, nicht Füllstand
  (`feedback_watchdog_that_only_fires_at_full`): ein Zähler „Frames vom Tap an den
  Gast zugestellt" pro Sekunde. Steht er, ist die Ringfüllung gleichgültig.
- `microvm benchvm` gegen einen LAN-Server auf dem Notebook. Zielmarke ist nicht
  Rekord, sondern **QEMU und Blech in derselben Grössenordnung**. Zum Einordnen:
  genestetes Linux mit slirp macht auf demselben Rechner 3–4 Gbit bei 0,44 ms,
  unsere Brücke 130–450 Mbit bei 3 ms (`project_netbench_coldstart`). Der Abstand
  ist die Rechnung, die offen bleibt.
- Das Messwerkzeug darf nicht stören und nicht selbst der Fehler sein
  (`feedback_diagnostics_must_not_disturb`). In dieser Jagd war es dreimal das
  Werkzeug: `swap(0)` auf Zähler mit zwei Lesern, `0` statt `--` beim ersten
  Aufruf, `benchvm` mit fest verdrahteten 150 MB und weggeworfenem Exit-Status.

## Nicht Ziel

IPv6 im Gast. Durchsatz-Feinschliff. GRO in den Schritten 1–5. Der
`VM_BIG_LOCK`-Umbau (eigenes Papier, `project_netbench_coldstart`).
`VIRTIO_NET_F_MTU` und die festverdrahtete 1514 — notieren, in diesem Umbau nicht
anfassen.

## Nebenbefunde, notiert, nicht Auftrag

- `ipv4::send` setzt nie DF und lässt die IP-Identification auf 0; RFC 6864
  verlangt bei DF=0 eine eindeutige ID.
- `tx_finish` verwirft eine synthetische ARP-/DNS-Antwort still bei vollem RX-Ring.
- `L3_UDP_IDLE_TICKS` ist flach 30 s; Linux' conntrack nimmt 180 s für beidseitig
  bestätigte Ströme. 65 QUIC-Flüsse, 61 davon abgeräumt.
- `l3_outbound` gibt immer `None` zurück (`nat.rs:776`) — der `Option`-Rückgabetyp
  ist Überbleibsel.

## Hausregeln, die hier schon Blut gekostet haben

- `feedback_second_session_in_the_same_tree` — nie `git add -A`, nie
  `./build.sh release`, wenn eine zweite Session im selben Baum arbeitet.
- `feedback_release_flow` — `./build.sh release` nach jedem Kernel-Commit, sonst
  ist jedes `update` beim Nutzer ein stiller Rückschritt.
- `feedback_test_on_hw` — NUC und Notebook schlagen QEMU. Messen, nicht raten.
- `feedback_port_completely_debug_never` — erst Umfang messen, dann ganze
  Funktionen portieren, dann messen.
- Jeder `unsafe`-Block braucht einen SAFETY-Kommentar. Kommentare knapp, englisch.
- Vor dem Commit: kann ein WASM-Modul durch diese Änderung aus seiner Sandbox?

## Einstieg

In dieser Reihenfolge lesen:

1. `memory/project_microvm_net_hw_hunt.md` — die Eingrenzung, und was schon
   ausgeschlossen ist
2. `memory/project_netbench_coldstart.md` — die Leiter: was Linux auf derselben
   Maschine kann
3. `kernel/src/microvm/devices/net_dataplane.rs` — der Kopf sagt, was gebaut
   werden sollte; der Code darunter etwas anderes
4. `kernel/src/microvm/devices/nat.rs` von oben nach unten
