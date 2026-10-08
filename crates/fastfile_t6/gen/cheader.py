"""bo2zm: a small C header reader for the T6 asset-structure header.

Reads type facts (typedefs, enums, structs, unions, members, declarators,
bit-fields, alignment attributes, #pragma pack) and computes the 32-bit MSVC
layout of every record. Development tool only: run on the developer's machine
against an OpenAssetTools checkout; nothing it reads is copied into the repo.
"""

import re

PTR_SIZE = 4

PRIMS = {
    # name: (size, align)
    'char': (1, 1), 'signed char': (1, 1), 'unsigned char': (1, 1),
    'bool': (1, 1),
    'short': (2, 2), 'unsigned short': (2, 2), 'short int': (2, 2), 'unsigned short int': (2, 2),
    'int': (4, 4), 'unsigned int': (4, 4), 'unsigned': (4, 4), 'signed int': (4, 4),
    'long': (4, 4), 'unsigned long': (4, 4), 'long int': (4, 4), 'unsigned long int': (4, 4),
    'long long': (8, 8), 'unsigned long long': (8, 8),
    'float': (4, 4), 'double': (8, 8),
    'void': (0, 1),
    'int8_t': (1, 1), 'uint8_t': (1, 1), 'int16_t': (2, 2), 'uint16_t': (2, 2),
    'int32_t': (4, 4), 'uint32_t': (4, 4), 'int64_t': (8, 8), 'uint64_t': (8, 8),
    'size_t': (4, 4), 'uintptr_t': (4, 4), 'intptr_t': (4, 4),
}

SIGNED_PRIMS = {'char', 'signed char', 'short', 'short int', 'int', 'signed int', 'long', 'long int',
                'long long', 'int8_t', 'int16_t', 'int32_t', 'int64_t', 'intptr_t'}
FLOAT_PRIMS = {'float', 'double'}


class ParseError(Exception):
    pass


# ---------------------------------------------------------------- lexing

TOKEN_RE = re.compile(r'''
    (?P<ws>\s+)
  | (?P<num>0[xX][0-9a-fA-F]+[uUlL]*|\d+\.\d*[fF]?|\d+[uUlL]*)
  | (?P<id>[A-Za-z_][A-Za-z_0-9]*)
  | (?P<op>::|<<|>>|==|!=|<=|>=|&&|\|\||[{}()\[\];,:*=<>+\-/%&|^~!?.#])
''', re.X)


def strip_comments(text):
    text = re.sub(r'/\*.*?\*/', lambda m: '\n' * m.group(0).count('\n'), text, flags=re.S)
    text = re.sub(r'//[^\n]*', '', text)
    return text


def preprocess(text):
    """Drop preprocessor lines and `#ifndef __zonecodegenerator` blocks (the
    namespace wrapper and static_asserts); keep `#pragma pack` as tokens."""
    out = []
    skip_depth = 0
    depth = 0
    for line in strip_comments(text).split('\n'):
        s = line.strip()
        if s.startswith('#'):
            d = s[1:].strip()
            if d.startswith(('if', 'ifdef', 'ifndef')):
                depth += 1
                if skip_depth == 0 and d.startswith('ifndef') and '__zonecodegenerator' in d:
                    skip_depth = depth
            elif d.startswith('endif'):
                if skip_depth == depth:
                    skip_depth = 0
                depth -= 1
            elif d.startswith('pragma') and 'pack' in d and skip_depth == 0:
                m = re.search(r'pack\s*\(\s*push\s*,\s*(\d+)\s*\)', d)
                if m:
                    out.append(' __pack_push ( %s ) ' % m.group(1))
                elif re.search(r'pack\s*\(\s*pop\s*\)', d):
                    out.append(' __pack_pop ')
                else:
                    raise ParseError('unsupported pragma: ' + s)
            out.append('')
            continue
        out.append('' if skip_depth else line)
    return '\n'.join(out)


def tokenize(text):
    toks = []
    pos = 0
    while pos < len(text):
        m = TOKEN_RE.match(text, pos)
        if not m:
            raise ParseError('bad character at %r' % text[pos:pos + 30])
        pos = m.end()
        kind = m.lastgroup
        if kind == 'ws':
            continue
        toks.append((kind, m.group(kind)))
    return toks


def parse_int(s):
    s = s.rstrip('uUlL')
    return int(s, 16) if s.lower().startswith('0x') else int(s)


# ---------------------------------------------------------------- model

class TypeDecl:
    """A base type plus declarator modifiers in C reading order from the
    name outward: ('ptr',) or ('arr', n). `T* a[4]` is [arr 4, ptr]."""

    def __init__(self, base, mods, is_const=False):
        self.base = base
        self.mods = mods
        self.is_const = is_const

    def __repr__(self):
        return 'TypeDecl(%s, %r)' % (self.base, self.mods)


class Member:
    def __init__(self, name, decl, bits=None, forced_align=None, anon=False):
        self.name = name
        self.decl = decl
        self.bits = bits
        self.forced_align = forced_align
        self.anon = anon
        self.offset = None
        self.bit_shift = None


class Record:
    def __init__(self, name, is_union, pack, forced_align, anon=False):
        self.name = name
        self.is_union = is_union
        self.pack = pack
        self.forced_align = forced_align
        self.members = []
        self.anon = anon
        self.size = None
        self.align = None          # MSVC alignment
        self.oat_align = None      # alignment used for zone allocations
        self.defined = True


class Enum:
    def __init__(self, name, size):
        self.name = name
        self.size = size
        self.values = {}


class Typedef:
    def __init__(self, name, decl, align_override=None):
        self.name = name
        self.decl = decl
        self.align_override = align_override


class Header:
    def __init__(self):
        self.records = {}      # name -> Record
        self.enums = {}        # name -> Enum
        self.typedefs = {}     # name -> Typedef
        self.constants = {}    # enum constant -> int
        self.order = []        # record names in definition order
        self._anon = 0

    def anon_name(self, parent, kind):
        self._anon += 1
        return '%s__anon%s%d' % (parent, kind, self._anon)


# ---------------------------------------------------------------- parsing

ALIGN_MACROS = {'tdef_align32', 'type_align32', 'tdef_align', 'type_align'}
IGNORED_ALIGN_MACROS = {'gcc_align32', 'gcc_align', 'tdef_align64', 'type_align64', 'gcc_align64'}


class Parser:
    def __init__(self, toks, header):
        self.t = toks
        self.i = 0
        self.h = header
        self.pack = [None]

    def peek(self, k=0):
        j = self.i + k
        return self.t[j][1] if j < len(self.t) else None

    def next(self):
        tok = self.t[self.i][1]
        self.i += 1
        return tok

    def expect(self, v):
        tok = self.next()
        if tok != v:
            raise ParseError('expected %r got %r near %r' % (v, tok, self.context()))
        return tok

    def context(self):
        return ' '.join(x[1] for x in self.t[max(0, self.i - 8):self.i + 8])

    def eof(self):
        return self.i >= len(self.t)

    # -- constant expressions (array sizes, enum values)
    def const_expr(self, stop):
        toks = []
        depth = 0
        while True:
            p = self.peek()
            if p is None:
                raise ParseError('unterminated constant expression')
            if depth == 0 and p in stop:
                break
            if p in '([':
                depth += 1
            elif p in ')]':
                depth -= 1
            toks.append(self.next())
        return eval_const(toks, self.h.constants)

    def attrs(self):
        """Alignment attributes before a type or record name."""
        forced = None
        while self.peek() in ALIGN_MACROS or self.peek() in IGNORED_ALIGN_MACROS:
            macro = self.next()
            self.expect('(')
            n = self.const_expr(')')
            self.expect(')')
            if macro in ALIGN_MACROS:
                forced = max(forced or 0, n)
        return forced

    def parse(self):
        while not self.eof():
            p = self.peek()
            if p == '__pack_push':
                self.next(); self.expect('(')
                n = int(self.next()); self.expect(')')
                self.pack.append(n)
            elif p == '__pack_pop':
                self.next()
                self.pack.pop()
            elif p == ';':
                self.next()
            elif p == 'typedef':
                self.typedef()
            elif p == 'enum':
                self.enum_def()
            elif p in ('struct', 'union'):
                self.record_or_forward()
            else:
                raise ParseError('unexpected %r near %r' % (p, self.context()))

    def typedef(self):
        self.expect('typedef')
        align = self.attrs()
        base, is_const = self.type_spec()
        name, mods = self.declarator()
        self.expect(';')
        self.h.typedefs[name] = Typedef(name, TypeDecl(base, mods, is_const), align)

    def enum_def(self):
        self.expect('enum')
        name = self.next()
        size = 4
        if self.peek() == ':':
            self.next()
            base, _ = self.type_spec()
            size = PRIMS[base][0]
        if self.peek() == ';':
            self.next()
            self.h.enums.setdefault(name, Enum(name, size))
            return
        e = Enum(name, size)
        self.expect('{')
        value = -1
        while self.peek() != '}':
            cname = self.next()
            if self.peek() == '=':
                self.next()
                value = self.const_expr((',', '}'))
            else:
                value += 1
            e.values[cname] = value
            self.h.constants[cname] = value
            if self.peek() == ',':
                self.next()
        self.expect('}')
        self.expect(';')
        self.h.enums[name] = e

    def record_or_forward(self):
        kind = self.next()
        forced = self.attrs()
        name = self.next()
        if self.peek() == ';':
            self.next()
            if name not in self.h.records:
                r = Record(name, kind == 'union', self.pack[-1], forced)
                r.defined = False
                self.h.records[name] = r
            return
        rec = self.record_body(name, kind == 'union', forced)
        self.expect(';')
        self.add_record(rec)

    def add_record(self, rec):
        old = self.h.records.get(rec.name)
        if old is not None and old.defined:
            raise ParseError('record defined twice: ' + rec.name)
        self.h.records[rec.name] = rec
        self.h.order.append(rec.name)

    def record_body(self, name, is_union, forced, anon=False):
        rec = Record(name, is_union, self.pack[-1], forced, anon)
        self.expect('{')
        while self.peek() != '}':
            self.member_decl(rec)
        self.expect('}')
        return rec

    def member_decl(self, rec):
        forced = self.attrs()
        if self.peek() in ('struct', 'union') and self.peek(1) == '{' or \
                self.peek() in ('struct', 'union') and self.peek(1) in ALIGN_MACROS:
            kind = self.next()
            inner_forced = self.attrs()
            inner = self.record_body(self.h.anon_name(rec.name, kind), kind == 'union', inner_forced, anon=True)
            self.add_record(inner)
            if self.peek() == ';':
                self.next()
                m = Member(inner.name.split('__')[-1], TypeDecl(inner.name, []), forced_align=forced, anon=True)
                rec.members.append(m)
                return
            while True:
                mname, mods = self.declarator()
                rec.members.append(Member(mname, TypeDecl(inner.name, mods), forced_align=forced))
                if self.peek() == ',':
                    self.next()
                    continue
                break
            self.expect(';')
            return
        base, is_const = self.type_spec()
        while True:
            mname, mods = self.declarator()
            bits = None
            if self.peek() == ':':
                self.next()
                bits = self.const_expr((',', ';'))
            rec.members.append(Member(mname, TypeDecl(base, mods, is_const), bits, forced))
            if self.peek() == ',':
                self.next()
                continue
            break
        self.expect(';')

    def type_spec(self):
        is_const = False
        words = []
        while True:
            p = self.peek()
            if p == 'const' or p == 'volatile':
                self.next()
                is_const = True
                continue
            if p in ('struct', 'union', 'enum'):
                self.next()
                words.append(self.next())
                continue
            if p in ('unsigned', 'signed', 'short', 'long', 'int', 'char', 'float', 'double', 'bool', 'void'):
                words.append(self.next())
                continue
            if not words and p is not None and re.match(r'[A-Za-z_]', p):
                words.append(self.next())
                continue
            break
        if not words:
            raise ParseError('no type near %r' % self.context())
        if len(words) > 1 or words[0] in PRIMS:
            name = ' '.join(words)
            name = {'signed': 'int', 'unsigned long int': 'unsigned long', 'signed char': 'signed char'}.get(name, name)
            if name not in PRIMS:
                raise ParseError('unknown primitive %r' % name)
            return name, is_const
        return words[0], is_const

    def declarator(self):
        nptr = 0
        while self.peek() in ('*', 'const'):
            if self.next() == '*':
                nptr += 1
        inner = []
        if self.peek() == '(':
            self.next()
            name, inner = self.declarator()
            self.expect(')')
        else:
            name = self.next()
            if not re.match(r'[A-Za-z_]', name):
                raise ParseError('bad declarator %r near %r' % (name, self.context()))
        suffix = []
        while self.peek() == '[':
            self.next()
            n = self.const_expr((']',))
            self.expect(']')
            suffix.append(('arr', n))
        return name, inner + suffix + [('ptr',)] * nptr


def eval_const(toks, constants):
    """Integer constant expression with C precedence."""
    from cexpr import parse_expr, eval_static
    ast = parse_expr(toks)
    return eval_static(ast, constants)


# ---------------------------------------------------------------- layout

class Layout:
    def __init__(self, header):
        self.h = header
        self.busy = set()

    def resolve(self, name):
        """Follow typedef names to the final base name; returns (base, mods
        prefix from typedefs, align override seen first)."""
        mods = []
        override = None
        while name in self.h.typedefs:
            td = self.h.typedefs[name]
            if override is None and td.align_override:
                override = td.align_override
            mods = mods + td.decl.mods
            name = td.decl.base
        return name, mods, override

    def base_size_align(self, name):
        """(size, msvc align, oat align) of a base type name."""
        if name in PRIMS:
            s, a = PRIMS[name]
            return s, a, a
        if name in self.h.enums:
            s = self.h.enums[name].size
            return s, s, s
        if name in self.h.typedefs:
            td = self.h.typedefs[name]
            s, a, oa = self.decl_size_align(td.decl)
            if td.align_override:
                return s, max(a, td.align_override), td.align_override
            return s, a, oa
        rec = self.h.records.get(name)
        if rec is None:
            raise ParseError('unknown type ' + name)
        if not rec.defined:
            raise ParseError('incomplete type ' + name)
        self.compute(rec)
        return rec.size, rec.align, rec.oat_align

    def decl_size_align(self, decl):
        if any(m[0] == 'ptr' for m in decl.mods):
            # Arrays outside the first pointer multiply the pointer size.
            n = 1
            for m in decl.mods:
                if m[0] == 'ptr':
                    break
                n *= m[1]
            return PTR_SIZE * n, PTR_SIZE, PTR_SIZE
        s, a, oa = self.base_size_align(decl.base)
        for m in decl.mods:
            s *= m[1]
        return s, a, oa

    def compute(self, rec):
        if rec.size is not None:
            return
        if rec.name in self.busy:
            raise ParseError('recursive embedding in ' + rec.name)
        self.busy.add(rec.name)
        pack = rec.pack
        msvc_align = 1
        oat_align = 0
        if rec.is_union:
            size = 0
            for m in rec.members:
                s, a, oa = self.decl_size_align(m.decl)
                if m.forced_align:
                    a = max(a, m.forced_align)
                    oa = m.forced_align
                m.offset = 0
                size = max(size, s)
                msvc_align = max(msvc_align, min(a, pack) if pack else a)
                oat_align = max(oat_align, oa)
        else:
            size = 0
            unit = None   # (unit start, unit size, bits used) for bit-fields
            for m in rec.members:
                s, a, oa = self.decl_size_align(m.decl)
                if m.forced_align:
                    a = max(a, m.forced_align)
                    oa = m.forced_align
                eff = min(a, pack) if pack else a
                if m.bits is not None:
                    if unit is not None and unit[1] == s and unit[2] + m.bits <= s * 8:
                        m.offset = unit[0]
                        m.bit_shift = unit[2]
                        unit = (unit[0], s, unit[2] + m.bits)
                    else:
                        size = align_up(size, eff)
                        m.offset = size
                        m.bit_shift = 0
                        unit = (size, s, m.bits)
                        size += s
                    msvc_align = max(msvc_align, eff)
                    oat_align = max(oat_align, oa)
                    continue
                unit = None
                size = align_up(size, eff)
                m.offset = size
                size += s
                msvc_align = max(msvc_align, eff)
                oat_align = max(oat_align, oa)
        if rec.forced_align:
            msvc_align = max(msvc_align, rec.forced_align)
            oat_align = rec.forced_align
        rec.size = align_up(size, msvc_align)
        rec.align = msvc_align
        rec.oat_align = oat_align or 1
        self.busy.discard(rec.name)


def align_up(v, a):
    return (v + a - 1) // a * a


def parse_header(text, header=None, enums_only=False):
    header = header or Header()
    text = preprocess(text)
    if enums_only:
        # Keep only `enum Name [: base] { ... };` declarations; the rest of
        # such a file may be C++ the tokenizer does not take.
        text = '\n'.join(m.group(0) for m in re.finditer(r'\benum\s+\w+\s*(:\s*[\w ]+)?\{[^}]*\}\s*;', text))
    toks = tokenize(text)
    Parser(toks, header).parse()
    return header
