# DSDT findings — HP Elite Dragonfly G1 (HPQOEM 861F)

DSDT.aml: 164579 bytes, verified (signature + internal length match).
Disassembled: DSDT.dsl (35768 lines) via `iasl -d`.

## Battery call chain (for `_BST` / `_BIF` / `_BIX`)

```
\_SB.BAT0._BST()                 -> BTST(0)            [line 9914 / 9880]
  BTST(arg0) {                   (BAT0-scope wrapper)
    ^PCI0.LPCB.EC0.BTST(arg0, 1)                       [fills NBST package]
    Return DerefOf(NBST[arg0])
  }
\_SB.BAT0._BIX()                 -> BTIX(0)            [line 9909]
\_SB.BAT1._BIF()                 -> BTIF(1)            [line 9934]   (BAT1 absent)
```

The real work: `\_SB.PCI0.LPCB.EC0.BTST(arg0, arg1)` [line 28770],
`EC0.BTIF(arg0)` [28665], `EC0.BTIX(arg0)` [28713].

### EC0.BTST(sel, force) — what it reads
```
if BSTA(1<<sel) == 0x0F: NBST[sel] = {0, -1, -1, -1}; return   # absent
Acquire(ECMX)
if ECRG {
  BSEL = sel                 # EC WRITE (battery select nibble @0x86)
  Local0 = BST_              # status nibble @0x99
  Local3 = BPR_              # present rate @0x9D
  NBST[sel][2] = BRC_        # remaining capacity @0xA1  <-- the % numerator
  NBST[sel][3] = BPV_        # present voltage @0xA5
}
Release(ECMX)
Local0 &= (GACS()==1 ? ~1 : ~2)    # AC present clears discharge/charge bit
NBST[sel][0] = Local0              # state: bit0=discharging, bit1=charging
... rate clamping ...
NBST[sel][1] = Local3
```
NBST result Package(4) = { State, PresentRate, RemainingCapacity(BRC_), Voltage(BPV_) }.

### EC0.BTIF(sel) — what it reads (full-charge denominator)
```
NBTI[sel][1] = BDC_   # design capacity   @0x89
NBTI[sel][2] = BFC_   # FULL CHARGE cap   @0x8D   <-- the % denominator
NBTI[sel][4] = BDV_   # design voltage    @0x95
```

## Battery EC fields are FLAT (no windowed/IndexField indirection!)

`OperationRegion(ECRM, EmbeddedControl, Zero, 0xFF)` [line 27893],
`Field(ECRM, ByteAcc, NoLock, Preserve)`. Byte offsets (from bit accumulation):

| Field | Offset | Width | Meaning |
|---|---|---|---|
| BSEL | 0x86 | 4b | battery select (write) |
| BDC  | 0x89 | 16 | design capacity (mAh) |
| BFC  | 0x8D | 16 | **full charge capacity (mAh) — denominator** |
| BDV  | 0x95 | 16 | design voltage |
| BST  | 0x99 | 4b  | status: bit0=discharging, bit1=charging |
| BPR  | 0x9D | 16 | present rate |
| BRC  | 0xA1 | 16 | **remaining capacity (mAh) — numerator** |
| BPV  | 0xA5 | 16 | present voltage |

`% = BRC * 100 / BFC` (units mAh). Matches the earlier hand-reversed offsets.
NOTE: the old "windowed MBER/SECP" conclusion was a red herring — those windows
(line 4775, IndexField ECMI@5449) are for OTHER subsystems, not the battery.

`ECRG` = Name(ECRG, Zero) @27841 — "EC region ready" gate, set to 1 once the EC
EmbeddedControl region is registered (_REG). Interpreter must have ECRG=1 (we own
the EC region, so set it or run _REG).

## Helper methods (all small, ~10-15 lines)
- `GACS()` -> UPAD(); Return ACST   (AC status, EC field)  [28490]
- `BTDR(x)` -> sets/returns NNBO     [28551]
- `BSTA(mask)` -> BTDR(1); GBAP(); returns 0x0F (absent) or 0x1F (present) [28565]
- `GBAP()` -> reads BATP via ECMX    [28501]
- `GBSS(a,b)` -> builds serial-date STRING (ToBCD/ISTR/Concatenate) — NOT needed
  for %, only fills NBST[10]. Can stub.

## Interpreter opcode coverage required (minimal, for _BST + _BIF)
- Method def + invocation (args Arg0..6, Locals Local0..7, Return)
- Mutex Acquire/Release  -> no-op stub (single-threaded)
- OperationRegion(EmbeddedControl) + Field (ByteAcc, bit-offset accumulation,
  Offset() skips, <8-bit fields) -> EC read/write via host fn
- Named integer objects (Name) read/write: ECRG, NNBO, NBAP, GACP, BATP, ACST...
- Package create, Index ([]), DerefOf, Store-to-index-element
- Store (=), If/Else/ElseIf
- Integer ops: + - * / << >> & | ~ (And/Or/Not/ShiftL/ShiftR/Add/Subtract/
  Multiply/Divide), comparisons (==, !=, <, >), LAnd/LOr
- (Optional, for GBSS serial) ToBCD, Concatenate, ISTR — stub-able

Everything is standard ACPI AML; no vendor opcodes. Interpreter is tractable.
```
