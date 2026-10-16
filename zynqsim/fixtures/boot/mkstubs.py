import struct, sys, os
out = sys.argv[1]
# --- stub FSBL: ELF32 LE ARM, one PT_LOAD at 0x0 (OCM), 'b .' loop
code = struct.pack('<4I', 0xEAFFFFFE, 0xEAFFFFFE, 0xEAFFFFFE, 0xEAFFFFFE)
ehsize, phsize = 52, 32
off = 0x1000
eh = b'\x7fELF' + bytes([1, 1, 1, 0]) + b'\0' * 8
eh += struct.pack('<HHIIIIIHHHHHH', 2, 40, 1, 0x0, ehsize, 0, 0x05000200, ehsize, phsize, 1, 40, 0, 0)
ph = struct.pack('<8I', 1, off, 0x0, 0x0, len(code), len(code), 5, 0x1000)
blob = eh + ph
blob += b'\0' * (off - len(blob)) + code
open(os.path.join(out, 'fsbl_stub.elf'), 'wb').write(blob)


# --- dummy PL bitstream (.bit container, Vivado layout, NOT a real design)
def field(tag, s):
    b = s.encode() + b'\0'
    return tag + struct.pack('>H', len(b)) + b


hdr = struct.pack('>H', 9) + bytes.fromhex('0ff00ff00ff00ff000') + struct.pack('>H', 1)
hdr += field(b'a', 'zs_dummy;UserID=0XFFFFFFFF;Version=2022.1')
hdr += field(b'b', '7z020clg400')
hdr += field(b'c', '2026/10/16')
hdr += field(b'd', '12:00:00')
words = [0xFFFFFFFF] * 8 + [0x000000BB, 0x11220044] + [0xFFFFFFFF] * 2 + [0xAA995566]
words += [0x20000000] * 64 + [0x30008001, 0x0000000D] + [0x20000000] * 16
data = b''.join(struct.pack('>I', w) for w in words)
hdr += b'e' + struct.pack('>I', len(data))
open(os.path.join(out, 'dummy.bit'), 'wb').write(hdr + data)
print('ok')
