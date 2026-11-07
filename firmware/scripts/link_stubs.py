#!/usr/bin/env python3
"""Generate non-installable ELF link interfaces from reviewed metadata; no vendor files needed."""
import argparse
import json
from pathlib import Path
import re
import struct
import tempfile

ROOT = Path(__file__).resolve().parents[1]
# ELF machine per camera model: 8 = MIPS, 40 = ARM.
MACHINES = {'mi360': 8, 'reolink-e1-zoom': 40, 'ds-kd8003-ime1': 40}


def validate(m):
    if m.get('schema') != 1 or m.get('machine') not in (8, 40):
        raise ValueError('Unsupported link-interface schema/architecture')
    if not 0 < len(m['symbols']) <= 10000:
        raise ValueError('Invalid symbol count')
    for s in m['symbols']:
        if not s['name'] or '\0' in s['name'] or not 0 <= s['size'] <= 16 * 1024 * 1024:
            raise ValueError('Invalid symbol name/extent')
        if s['info'] & 15 not in (0, 1, 2, 3, 6) or s['info'] >> 4 not in (1, 2):
            raise ValueError('Unsupported symbol type/binding')
    for name in m['needed'] + ([m['soname']] if m['soname'] else []):
        if not re.fullmatch(r'[A-Za-z0-9_+.-]+', name) or name in ('.', '..'):
            raise ValueError('Unsafe library identity')
    abi = m['mips_abiflags']
    if abi is not None and (m['machine'] != 8 or len(bytes.fromhex(abi)) != 24):
        raise ValueError('Invalid MIPS ABI attributes')


class Strings:
    def __init__(self): self.data, self.offsets = bytearray(b'\0'), {'': 0}
    def add(self, text):
        if text not in self.offsets:
            self.offsets[text] = len(self.data)
            self.data.extend(text.encode() + b'\0')
        return self.offsets[text]


def generate(m):
    validate(m)
    strings = Strings()
    sections = [dict(name='', type=0, flags=0, data=b'', size=0, align=0, link=0, info=0, entsize=0)]
    def section(name, kind, flags, data=b'', *, size=None, align=4, link=0, info=0, entsize=0):
        sections.append(dict(name=name, type=kind, flags=flags, data=data,
                             size=len(data) if size is None else size, align=align,
                             link=link, info=info, entsize=entsize))
        return len(sections) - 1
    # No program headers and no executable implementation. These sections only
    # classify symbols for the static linker; NOBITS consumes no file payload.
    text = section('.text', 8, 6, size=max([s['size'] for s in m['symbols'] if s['info'] & 15 == 2] or [1]))
    data = section('.data', 8, 3, size=sum(s['size'] + 16 for s in m['symbols']))
    tls = section('.tdata', 8, 0x403, size=sum(s['size'] + 16 for s in m['symbols'] if s['info'] & 15 == 6))
    symbols, versions = bytearray(16), [0]
    data_offset = tls_offset = 0
    for s in m['symbols']:
        kind = s['info'] & 15
        value = 0
        if not s['defined']: index = 0
        elif s['absolute']: index, value = 0xfff1, s['absolute_value']
        elif kind == 6:
            index, value = tls, tls_offset
            tls_offset += (s['size'] + 15) // 16 * 16
        elif kind == 2: index = text
        else:
            index, value = data, data_offset
            data_offset += (s['size'] + 15) // 16 * 16
        symbols.extend(struct.pack('<IIIBBH', strings.add(s['name']), value, s['size'],
                                   s['info'], s['other'], index))
        versions.append(s['version'])
    dynstr = section('.dynstr', 3, 2, align=1)
    dynsym = section('.dynsym', 11, 2, bytes(symbols), link=dynstr, info=1, entsize=16)
    count = len(versions)
    # A valid one-bucket SysV table is sufficient for link-only stubs.
    hashes = [1, count, 1 if count > 1 else 0, 0] + list(range(2, count)) + ([0] if count > 1 else [])
    hash_section = section('.hash', 5, 2, struct.pack('<' + 'I' * len(hashes), *hashes), link=dynsym, entsize=4)
    version_section = definition_section = requirement_section = None
    if m['versioned']:
        version_section = section('.gnu.version', 0x6fffffff, 2, struct.pack('<' + 'H' * count, *versions), link=dynsym, entsize=2, align=2)
    if m['definitions']:
        blob = bytearray()
        for i, d in enumerate(m['definitions']):
            length = 20 + 8 * len(d['names'])
            blob.extend(struct.pack('<HHHHIII', 1, d['flags'], d['index'], len(d['names']), d['hash'], 20,
                                    length if i + 1 < len(m['definitions']) else 0))
            for j, name in enumerate(d['names']):
                blob.extend(struct.pack('<II', strings.add(name), 8 if j + 1 < len(d['names']) else 0))
        definition_section = section('.gnu.version_d', 0x6ffffffd, 2, bytes(blob), link=dynstr, info=len(m['definitions']))
    if m['requirements']:
        blob = bytearray()
        for i, r in enumerate(m['requirements']):
            length = 16 + 16 * len(r['symbols'])
            blob.extend(struct.pack('<HHIII', 1, len(r['symbols']), strings.add(r['library']), 16,
                                    length if i + 1 < len(m['requirements']) else 0))
            for j, s in enumerate(r['symbols']):
                blob.extend(struct.pack('<IHHII', s['hash'], s['flags'], s['index'], strings.add(s['name']),
                                        16 if j + 1 < len(r['symbols']) else 0))
        requirement_section = section('.gnu.version_r', 0x6ffffffe, 2, bytes(blob), link=dynstr, info=len(m['requirements']))
    if m['mips_abiflags']:
        section('.MIPS.abiflags', 0x7000002a, 2, bytes.fromhex(m['mips_abiflags']), align=8, entsize=24)
    tags = [(1, strings.add(n)) for n in m['needed']]
    if m['soname']: tags.append((14, strings.add(m['soname'])))
    # Patch dynamic pointers once section addresses are known.
    pointers = [(5, dynstr), (6, dynsym), (4, hash_section)]
    if version_section: pointers.append((0x6ffffff0, version_section))
    if definition_section:
        pointers.append((0x6ffffffc, definition_section))
        tags.append((0x6ffffffd, len(m['definitions'])))
    if requirement_section:
        pointers.append((0x6ffffffe, requirement_section))
        tags.append((0x6fffffff, len(m['requirements'])))
    tags.extend([(10, len(strings.data)), (11, 16)])
    dynamic = section('.dynamic', 6, 3, bytes(8 * (len(tags) + len(pointers) + 1)), link=dynstr, entsize=8)
    sections[dynstr].update(data=bytes(strings.data), size=len(strings.data))
    names = Strings()
    for s in sections: names.add(s['name'])
    names.add('.shstrtab')
    shstr = section('.shstrtab', 3, 0, bytes(names.data), align=1)
    blob = bytearray(52)
    for s in sections[1:]:
        alignment = max(s['align'], 1)
        blob.extend(bytes((-len(blob)) % alignment))
        s['offset'] = len(blob)
        s['address'] = len(blob) if s['flags'] & 2 else 0
        if s['type'] != 8: blob.extend(s['data'])
    dynamic_data = b''.join(struct.pack('<II', tag, value) for tag, value in
                            tags + [(tag, sections[index]['address']) for tag, index in pointers] + [(0, 0)])
    start = sections[dynamic]['offset']
    blob[start:start + len(dynamic_data)] = dynamic_data
    blob.extend(bytes((-len(blob)) % 4))
    shoff = len(blob)
    for s in sections:
        blob.extend(struct.pack('<10I', names.add(s['name']), s['type'], s['flags'], s.get('address', 0),
                                s.get('offset', 0), s['size'], s['link'], s['info'], s['align'], s['entsize']))
    ident = b'\x7fELF\x01\x01\x01' + bytes([m['osabi'], m['abi_version']]) + bytes(7)
    blob[:52] = ident + struct.pack('<HHIIIIIHHHHHH', 3, m['machine'], 1, 0, 0, shoff,
                                   m['flags'], 52, 0, 0, 40, len(sections), shstr)
    return bytes(blob)



def install(model, output):
    """Write only deterministic link interfaces into a dedicated generated directory."""
    if model not in MACHINES:
        raise ValueError('Unknown camera model')
    manifests = ROOT / 'abi' / model
    output = Path(output)
    output.mkdir(parents=True, exist_ok=True)
    paths = []
    for path in sorted(manifests.glob('*.json')):
        m = json.loads(path.read_text())
        filename = m.pop('filename')
        if not re.fullmatch(r'[A-Za-z0-9_+.-]+', filename) or filename in ('.', '..'):
            raise ValueError('Unsafe output name')
        if m['machine'] != MACHINES[model]:
            raise ValueError('Wrong camera architecture')
        data = generate(m)
        target = output / filename
        if not target.is_file() or target.read_bytes() != data:
            with tempfile.NamedTemporaryFile(dir=output, delete=False) as temporary:
                temporary.write(data)
                scratch = Path(temporary.name)
            scratch.replace(target)
        paths.append(target)
    if not paths:
        raise ValueError('No link-interface manifests found')
    return paths


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('model', choices=sorted(MACHINES))
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    paths = install(args.model, args.output)
    print(f'Generated {len(paths)} link-only interfaces; never install these files')


if __name__ == '__main__':
    main()
