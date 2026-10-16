# Ton an den Monitor (HDMI/DP-Audio) — Plan

Stand 2026-10-16. Wunsch Florian: Ton über das Monitorkabel, neben dem
analogen Ausgang. Alles nach Linux 6.18.55 1:1.

## Befund am NUC

- Ein HDA-Controller, `00:1f.3 8086:a0c8` (Tiger Lake). Analoger Codec und
  Intels HDMI/DP-Codec hängen am selben Link; der zweite erscheint nur,
  wenn die Anzeige ihren Audioteil einschaltet und den Codec weckt.
- Der Monitor hängt am HDMI-Anschluss, das Bild kommt aber aus **Port D
  (TC1, legacy) als DisplayPort** (`output_types 0x80`), 3840x2160 mit
  DSC, `audio 0`. Auf der Platine sitzt also ein DP→HDMI-Wandler: der Ton
  geht als **DP-Audio** hinaus, der Wandler macht HDMI daraus.
- Port B (natives HDMI, DDC-Pin 2) ist aus.
- `intel_dp_get_dpcd skipped: AUX transfers are not ported` — ohne AUX
  gibt es weder DPCD noch EDID, also keine Ton-Beschreibung (ELD).

## Linux-Bild

- i915 liest die EDID, baut daraus die ELD (`drm_edid_to_eld`) und setzt
  `has_audio`.
- `intel_audio_codec_enable` (hsw-Zweig, DP): DSC-Workaround bei 4K,
  Präsenz melden (`AUD_PIN_ELD_CP_VLD`), Zeitstempel/Maud/Naud von der
  Hardware (`AUD_CFG` mit `N_VALUE_INDEX`).
- Die ELD geht nicht über den Hardwarepuffer, sondern über die
  **audio component** an snd-hda: `get_power` (Audio-Stromdomäne,
  `AUD_FREQ_CNTRL`, `AUD_PIN_BUF_ENABLE`), `codec_wake_override`,
  `get_eld`, `sync_audio_rate`, `pin_eld_notify`.
- snd-hda (`sound/hda/codecs/hdmi/intelhdmi.c` + `hdmi.c`): alle Pins
  freischalten (`intel_haswell_enable_all_pins`), DP1.2-Fixup, Pin↔Port,
  Wandler wählen, Kanäle, Audio-Infoframe (DIP über Codec-Verben).

## Schritte

1. **DP-AUX** (`intel_dp_aux.c`, `drm_dp_helper.c`): native Transfers
   (DPCD) und I²C-über-AUX. Beweis am Gerät: DPCD-Revision und
   Wandler-Fähigkeiten im Log.
2. **EDID über AUX und ELD** (`drm_edid.c`: Basisblock + CEA-Erweiterung,
   `drm_edid_to_eld`). Beweis: Monitorname und Audio-Formate im Log.
3. **Audio-Freischaltung DP** (`intel_audio.c`, hsw-Zweig, DSC-WA,
   Stromdomäne, Codec-Wecken). Beweis: audio_hda sieht den zweiten Codec
   in `STATESTS`.
4. **Kernel-Kanal** Anzeige ↔ Ton (Gegenstück der audio component):
   die GPU-Seite legt Präsenz und ELD je Port ab, audio_hda liest sie und
   meldet die Abtastrate; beide Seiten nur mit ihren eigenen Rechten.
5. **audio_hda**: Intel-HDMI/DP-Codec und Wahl des Ausgangs
   (`set audio_out hdmi|analog`).

Nebenbei, mit dem ersten Release: was Linux als `drm_dbg`/`drm_dbg_kms`
führt, schreibt das Modul nur noch mit `bootlog verbose`.
