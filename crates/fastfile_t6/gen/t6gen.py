"""bo2zm: generate crates/fastfile_t6/src/walk_gen.rs, the T6 zone walk.

Inputs (read-only, on the developer's machine): the T6 asset-structure
header and zone-load rules of an OpenAssetTools checkout. Facts only: type
layouts (cross-checked against 32-bit MSVC by layoutcheck.py), and per member
the counts, conditions, blocks and reuse rules. The output is our own Rust.

usage: python t6gen.py <oat checkout> <out walk_gen.rs> [<layout check .cpp>]
"""

import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import cheader  # noqa: E402
import layoutcheck  # noqa: E402
from cexpr import ExprError, parse_expr  # noqa: E402

PTR = 4

# XAssetType -> asset structure, as the T6 content loader dispatches.
ASSET_DISPATCH = [
    ('PhysPreset', 'PhysPreset'),
    ('PhysConstraints', 'PhysConstraints'),
    ('DestructibleDef', 'DestructibleDef'),
    ('XAnimParts', 'XAnimParts'),
    ('XModel', 'XModel'),
    ('Material', 'Material'),
    ('TechniqueSet', 'MaterialTechniqueSet'),
    ('Image', 'GfxImage'),
    ('Sound', 'SndBank'),
    ('SoundPatch', 'SndPatch'),
    ('ClipMap', 'clipMap_t'),
    ('ClipMapPvs', 'clipMap_t'),
    ('ComWorld', 'ComWorld'),
    ('GameWorldSp', 'GameWorldSp'),
    ('GameWorldMp', 'GameWorldMp'),
    ('MapEnts', 'MapEnts'),
    ('GfxWorld', 'GfxWorld'),
    ('LightDef', 'GfxLightDef'),
    ('Font', 'Font_s'),
    ('FontIcon', 'FontIcon'),
    ('MenuList', 'MenuList'),
    ('Menu', 'menuDef_t'),
    ('Localize', 'LocalizeEntry'),
    ('Weapon', 'WeaponVariantDef'),
    ('Attachment', 'WeaponAttachment'),
    ('AttachmentUnique', 'WeaponAttachmentUnique'),
    ('WeaponCamo', 'WeaponCamo'),
    ('SndDriverGlobals', 'SndDriverGlobals'),
    ('Fx', 'FxEffectDef'),
    ('ImpactFx', 'FxImpactTable'),
    ('RawFile', 'RawFile'),
    ('StringTable', 'StringTable'),
    ('Leaderboard', 'LeaderboardDef'),
    ('XGlobals', 'XGlobals'),
    ('Ddl', 'ddlRoot_t'),
    ('Glasses', 'Glasses'),
    ('EmblemSet', 'EmblemSet'),
    ('ScriptParseTree', 'ScriptParseTree'),
    ('KeyValuePairs', 'KeyValuePairs'),
    ('VehicleDef', 'VehicleDef'),
    ('MemoryBlock', 'MemoryBlock'),
    ('AddonMapEnts', 'AddonMapEnts'),
    ('Tracer', 'TracerDef'),
    ('SkinnedVerts', 'SkinnedVertsDef'),
    ('Qdb', 'Qdb'),
    ('Slug', 'Slug'),
    ('FootstepTable', 'FootstepTableDef'),
    ('FootstepFxTable', 'FootstepFXTableDef'),
    ('ZBarrier', 'ZBarrierDef'),
]

# Asset structure -> AssetType reported to the sink when it loads (nested
# loads included). clipMap_t serves two pools; the PVS one is what PC uses.
ASSET_OF_STRUCT = {}
for _variant, _struct in ASSET_DISPATCH:
    ASSET_OF_STRUCT.setdefault(_struct, _variant)
ASSET_OF_STRUCT['clipMap_t'] = 'ClipMapPvs'


class GenError(Exception):
    pass


# ---------------------------------------------------------------- model

class Block:
    def __init__(self, kind, name, index, default):
        self.kind = kind          # temp | runtime | delay | normal
        self.name = name
        self.index = index
        self.default = default


class Ptr_:
    """Count rules of one pointer modifier."""

    def __init__(self):
        self.default = None       # resolved expr, None = 1
        self.by_index = {}        # combined array index -> resolved expr


class MInfo:
    def __init__(self, parent, member):
        self.parent = parent
        self.m = member
        self.name = member.name
        self.mods = member.decl.mods
        self.base = member.decl.base
        self.type = None          # SInfo of the base record, if any
        self.is_string = False
        self.is_scriptstring = False
        self.reusable = False
        self.block = None
        self.condition = None     # resolved expr or ('never',) / ('always',)
        self.alloc_align = None   # resolved expr
        self.ptrs = {}            # modifier index -> Ptr_
        self.dynsize = {}         # modifier index -> resolved expr (arraysize)
        self.dyncount = {}        # modifier index -> resolved expr (arraycount)
        self.is_leaf = True


class SInfo:
    def __init__(self, rec, ident):
        self.rec = rec
        self.name = rec.name
        self.id = ident
        self.is_union = rec.is_union
        self.asset = None
        self.block = None
        self.alloc_align = None
        self.members = []
        self.ordered = []
        self.is_leaf = True
        self.ambiguous = False

    def member(self, name):
        for mi in self.members:
            if mi.name == name and not mi.m.anon:
                return mi
        return None


# ---------------------------------------------------------------- commands

def read_commands(path):
    out = []
    base = os.path.dirname(path)
    for line in open(path, encoding='utf-8').read().split('\n'):
        m = re.match(r'\s*#include\s+"([^"]+)"', line)
        if m:
            out.append(read_commands(os.path.join(base, m.group(1))))
        else:
            out.append(line)
    return '\n'.join(out)


def split_statements(text):
    text = re.sub(r'//[^\n]*', '', text)
    toks = cheader.tokenize(text)
    stmts = []
    cur = []
    for kind, tok in toks:
        if tok == ';':
            if cur:
                stmts.append(cur)
            cur = []
        else:
            cur.append(tok)
    if cur:
        raise GenError('unterminated statement: %r' % cur)
    return stmts


class Model:
    def __init__(self, header):
        self.h = header
        self.lay = cheader.Layout(header)
        for name in header.order:
            self.lay.compute(header.records[name])
        self.infos = {}
        for name in header.order:
            self.infos[name] = SInfo(header.records[name], len(self.infos))
        for si in self.infos.values():
            for m in si.rec.members:
                mi = MInfo(si, m)
                base, tmods, _ = self.lay.resolve(m.decl.base)
                if base in self.infos and not tmods:
                    mi.type = self.infos[base]
                si.members.append(mi)
            si.ordered = list(si.members)
        self.blocks = {}
        self.in_use = None

    # -- name resolution -------------------------------------------------
    def member_chain(self, si, segs):
        chain = []
        cur = si
        for seg in segs:
            if cur is None:
                return None
            mi = cur.member(seg)
            if mi is None:
                return None
            chain.append(mi)
            cur = mi.type
        return chain

    def typename_members(self, name):
        """GetTypenameAndMembersFromTypename: the in-use type first, then the
        shortest type-name prefix."""
        segs = name.split('::')
        if self.in_use is not None:
            chain = self.member_chain(self.in_use, segs)
            if chain is not None:
                return self.in_use, chain
        for i in range(1, len(segs) + 1):
            tname = '::'.join(segs[:i])
            si = self.infos.get(tname)
            if si is not None:
                chain = self.member_chain(si, segs[i:])
                if chain is None:
                    return None
                return si, chain
        return None

    def resolve_expr(self, ast, current):
        k = ast[0]
        if k == 'num':
            return ast
        if k == 'name':
            name, idx = ast[1], ast[2]
            idx = [self.resolve_expr(x, current) for x in idx]
            if name in self.h.constants and not idx:
                return ('num', self.h.constants[name])
            found = self.typename_members(name)
            if found is None and current is not None and current is not self.in_use:
                chain = self.member_chain(current, name.split('::'))
                if chain is not None:
                    found = (current, chain)
            if found is None or not found[1]:
                raise GenError('unknown operand %s (use %s)' % (name, self.in_use and self.in_use.name))
            return ('field', found[0], found[1], idx)
        if k == 'un':
            return ('un', ast[1], self.resolve_expr(ast[2], current))
        if k == 'bin':
            return ('bin', ast[1], self.resolve_expr(ast[2], current), self.resolve_expr(ast[3], current))
        if k == 'tern':
            return ('tern',) + tuple(self.resolve_expr(x, current) for x in ast[1:])
        raise GenError(repr(ast))

    def expr_from(self, toks, current):
        if toks in (['never'], ['always']):
            return (toks[0],)
        return self.resolve_expr(parse_expr(toks), current)

    @staticmethod
    def take_typename(toks, i):
        name = toks[i]
        i += 1
        while i + 1 < len(toks) and toks[i] == '::':
            name += '::' + toks[i + 1]
            i += 2
        return name, i

    def target(self, name):
        found = self.typename_members(name)
        if found is None:
            raise GenError('unknown target ' + name)
        return found

    # -- statements -------------------------------------------------------
    def apply(self, st):
        head = st[0]
        if head in ('game', 'wordsize', 'architecture'):
            return
        if head == 'asset':
            si = self.infos[st[1]]
            si.asset = st[2]
            return
        if head == 'block':
            kind, name = st[1], st[2]
            default = len(st) > 3 and st[3] == 'default'
            self.blocks[name] = Block(kind, name, len(self.blocks), default)
            return
        if head == 'use':
            name, _ = self.take_typename(st, 1)
            si, chain = self.target(name) if '::' in name else (self.infos[name], [])
            self.in_use = chain[-1].type if chain else si
            return
        if head == 'reorder':
            self.reorder(st)
            return
        if head != 'set':
            raise GenError('unknown statement %r' % st)
        verb = st[1]
        if verb == 'action' or verb == 'name':
            return
        if verb == 'block':
            if len(st) == 3:
                self.in_use.block = self.blocks[st[2]]
                return
            name, i = self.take_typename(st, 2)
            si, chain = self.target(name)
            blk = self.blocks[st[i]]
            if chain:
                chain[-1].block = blk
            else:
                si.block = blk
            return
        if verb in ('string', 'scriptstring', 'reusable'):
            name, _ = self.take_typename(st, 2)
            si, chain = self.target(name)
            mi = chain[-1]
            setattr(mi, {'string': 'is_string', 'scriptstring': 'is_scriptstring', 'reusable': 'reusable'}[verb], True)
            return
        if verb == 'assetref':
            return
        if verb == 'allocalign':
            name, i = self.take_typename(st, 2)
            si, chain = self.target(name)
            ex = self.expr_from(st[i:], si)
            if chain:
                chain[-1].alloc_align = ex
            else:
                si.alloc_align = ex
            return
        if verb == 'condition':
            name, i = self.take_typename(st, 2)
            si, chain = self.target(name)
            chain[-1].condition = self.expr_from(st[i:], si)
            return
        if verb == 'count':
            i = 2
            skip = 0
            while st[i] == '*':
                skip += 1
                i += 1
            name, i = self.take_typename(st, i)
            si, chain = self.target(name)
            mi = chain[-1]
            idx = []
            while st[i] == '[':
                tok = st[i + 1]
                idx.append(int(tok) if tok[0].isdigit() else self.h.constants[tok])
                assert st[i + 2] == ']'
                i += 3
            ex = self.expr_from(st[i:], si)
            pidx = [k for k, m in enumerate(mi.mods) if m[0] == 'ptr'][skip]
            p = mi.ptrs.setdefault(pidx, Ptr_())
            if not idx:
                p.default = ex
            else:
                sizes = [m[1] for m in mi.mods if m[0] == 'arr']
                combined = 0
                for d, v in enumerate(idx):
                    stride = 1
                    for s in sizes[d + 1:]:
                        stride *= s
                    combined += stride * v
                p.by_index[combined] = ex
            return
        if verb in ('arraysize', 'arraycount'):
            name, i = self.take_typename(st, 2)
            si, chain = self.target(name)
            mi = chain[-1]
            ex = self.expr_from(st[i:], si)
            aidx = [k for k, m in enumerate(mi.mods) if m[0] == 'arr'][0]
            (mi.dynsize if verb == 'arraysize' else mi.dyncount)[aidx] = ex
            return
        raise GenError('unknown set verb %r' % st)

    def reorder(self, st):
        i = 1
        if st[i] != ':':
            name, i = self.take_typename(st, i)
            si, chain = self.target(name) if '::' in name or name not in self.infos else (self.infos[name], [])
            si = chain[-1].type if chain else si
        else:
            si = self.in_use
        assert st[i] == ':'
        i += 1
        find_first = None
        if st[i:i + 3] == ['.', '.', '.']:
            i += 3
            find_first = st[i]
            i += 1
        names = st[i:]
        old = list(si.ordered)
        new = []
        for n in names:
            mi = next((m for m in old if m.name == n), None)
            if mi is None:
                raise GenError('reorder: no member %s in %s' % (n, si.name))
            old.remove(mi)
            new.append(mi)
        lead = []
        if find_first is not None:
            while old:
                mi = old.pop(0)
                lead.append(mi)
                if mi.name == find_first:
                    break
        si.ordered = lead + new + old


# ---------------------------------------------------------------- analysis

def static_value(ex):
    if ex is None:
        return 1
    if ex[0] == 'num':
        return ex[1]
    if ex[0] == 'never':
        return 0
    if ex[0] == 'always':
        return 1
    return None


def ignored(mi):
    return mi.condition is not None and static_value(mi.condition) == 0


def has_dynamic_size(mi):
    return bool(mi.dynsize)


def contains_ptr(mi):
    return any(m[0] == 'ptr' for m in mi.mods)


def ptr_default(mi, k):
    p = mi.ptrs.get(k)
    return p.default if p else None


def compute_leafs(model):
    done = {}

    def is_leaf(si, stack=()):
        if si.name in done:
            return done[si.name]
        for mi in si.ordered:
            if ignored(mi):
                continue
            if mi.is_scriptstring or mi.is_string:
                done[si.name] = False
                return False
            for k, mod in enumerate(mi.mods):
                if mod[0] == 'ptr' and static_value(ptr_default(mi, k)) != 0:
                    done[si.name] = False
                    return False
            if has_dynamic_size(mi):
                done[si.name] = False
                return False
            if mi.type is not None and mi.type is not si and mi.type.name not in stack:
                if not is_leaf(mi.type, stack + (si.name,)):
                    done[si.name] = False
                    return False
        done[si.name] = True
        return True

    for si in model.infos.values():
        si.is_leaf = is_leaf(si)
    for si in model.infos.values():
        for mi in si.members:
            leaf = not (mi.is_string or mi.is_scriptstring)
            if mi.type is not None and not mi.type.is_leaf:
                leaf = False
            for k, mod in enumerate(mi.mods):
                if mod[0] == 'ptr' and static_value(ptr_default(mi, k)) != 0:
                    leaf = False
            if has_dynamic_size(mi):
                leaf = False
            mi.is_leaf = leaf


def fix_unions(model):
    """A union's single non-leaf member without a condition becomes its last
    member, so it reads as the `else` of the condition chain."""
    for si in model.infos.values():
        if not si.is_union:
            continue
        bare = [m for m in si.ordered if m.condition is None and not m.is_leaf]
        if len(bare) > 1 and not si.is_leaf:
            si.ambiguous = True
        if len(bare) == 1:
            si.ordered.remove(bare[0])
            si.ordered.append(bare[0])


def is_dynamic_member(mi):
    if has_dynamic_size(mi):
        return True
    return not contains_ptr(mi) and mi.type is not None and dynamic_member(mi.type) is not None


def dynamic_member(si):
    for mi in si.ordered:
        if is_dynamic_member(mi):
            return mi
    return None


def after_partial_load(mi):
    if is_dynamic_member(mi):
        return True
    return mi.parent.is_union and dynamic_member(mi.parent) is not None


def needs_treatment(mi):
    if ignored(mi):
        return False
    return (mi.is_string or contains_ptr(mi) or (mi.type is not None and not mi.type.is_leaf)
            or after_partial_load(mi))


def used_members(si):
    if si.is_union and dynamic_member(si) is not None:
        return [m for m in si.ordered if not ignored(m)]
    return [m for m in si.ordered if not m.is_leaf and not ignored(m)]


# ---------------------------------------------------------------- emission

class Emitter:
    def __init__(self, model):
        self.model = model
        self.blocks = model.blocks
        self.default_normal = next(b for b in self.blocks.values() if b.kind == 'normal' and b.default)
        self.lines = []
        self.var = 0
        self.need = set()

    def fresh(self, stem):
        self.var += 1
        return '%s%d' % (stem, self.var)

    # -- types -----------------------------------------------------------
    def base_size(self, name):
        return self.model.lay.base_size_align(name)[0]

    def oat_align_of_base(self, name):
        return self.model.lay.base_size_align(name)[2]

    def struct_alloc_align(self, si):
        if si.alloc_align is not None:
            v = static_value(si.alloc_align)
            if v is None:
                raise GenError('dynamic struct allocalign on ' + si.name)
            return v
        return si.rec.oat_align

    def kind_of(self, base):
        b, mods, _ = self.model.lay.resolve(base)
        if mods:
            raise GenError('field read through array typedef ' + base)
        if b in self.model.h.enums:
            size = self.model.h.enums[b].size
            return {1: 'U8', 2: 'U16', 4: 'I32'}[size]
        if b in cheader.FLOAT_PRIMS:
            return 'F32'
        size = cheader.PRIMS[b][0]
        signed = b in cheader.SIGNED_PRIMS
        return {(1, False): 'U8', (1, True): 'I8', (2, False): 'U16', (2, True): 'I16',
                (4, False): 'U32', (4, True): 'I32', (8, False): 'U64', (8, True): 'I64'}[(size, signed)]

    # -- expressions -----------------------------------------------------
    def expr(self, ex):
        k = ex[0]
        if k == 'num':
            return '(%di64)' % ex[1]
        if k == 'never':
            return '0i64'
        if k == 'always':
            return '1i64'
        if k == 'field':
            si, chain, idx = ex[1], ex[2], ex[3]
            off = 0
            hop = None
            for mi in chain[:-1]:
                if mi.mods == [('ptr',)] and hop is None:
                    # One pointer hop (`model::numVerts` through SSkinModel*):
                    # read the already-loaded pointer, then the field behind it.
                    hop = off + mi.m.offset
                    off = 0
                    continue
                if contains_ptr(mi) or mi.mods:
                    raise GenError('field path through pointer/array: ' + '::'.join(m.name for m in chain))
                off += mi.m.offset
            last = chain[-1]
            off += last.m.offset
            arr = [m[1] for m in last.mods if m[0] == 'arr']
            if contains_ptr(last):
                raise GenError('field read of pointer ' + last.name)
            if len(idx) != len(arr):
                raise GenError('index count mismatch on ' + last.name)
            elem = self.base_size(last.base)
            off_expr = str(off)
            for d, ix in enumerate(idx):
                stride = elem
                for s in arr[d + 1:]:
                    stride *= s
                off_expr += ' + (%s as usize) * %d' % (self.expr(ix), stride)
            if last.m.bits is not None:
                raise GenError('field read of bit-field ' + last.name)
            if hop is not None:
                return 'w.rd_through(%d, %d, %s, Kind::%s)?' % (si.id, hop, off_expr, self.kind_of(last.base))
            return 'w.rd(%d, %s, Kind::%s)?' % (si.id, off_expr, self.kind_of(last.base))
        if k == 'un':
            a = self.expr(ex[2])
            return {'!': '((%s == 0) as i64)', '~': '(!%s)', '-': '(-%s)', '+': '(%s)'}[ex[1]] % a
        if k == 'bin':
            op, a, b = ex[1], self.expr(ex[2]), self.expr(ex[3])
            if op in ('==', '!=', '<', '<=', '>', '>='):
                return '((%s %s %s) as i64)' % (a, op, b)
            if op == '&&':
                return '(((%s != 0) && (%s != 0)) as i64)' % (a, b)
            if op == '||':
                return '(((%s != 0) || (%s != 0)) as i64)' % (a, b)
            if op == '/':
                return 'Walker::div(%s, %s)?' % (a, b)
            if op == '%':
                return 'Walker::rem(%s, %s)?' % (a, b)
            if op in ('<<', '>>'):
                return '(%s %s (%s as u32))' % (a, op, b)
            return '(%s %s %s)' % (a, op, b)
        if k == 'tern':
            return '(if %s != 0 { %s } else { %s })' % (self.expr(ex[1]), self.expr(ex[2]), self.expr(ex[3]))
        raise GenError(repr(ex))

    def cond(self, ex):
        return '%s != 0' % self.expr(ex)

    # -- classification (DeclarationModifierComputations) ----------------
    def combined_index(self, mi, indices):
        sizes = []
        for m in mi.mods:
            if m[0] != 'arr':
                break
            sizes.append(m[1])
        per = [1] * len(sizes)
        cur = 1
        for i in range(len(sizes), 0, -1):
            per[i - 1] = cur
            cur *= sizes[i - 1]
        return sum(per[d] * v for d, v in enumerate(indices))

    def count_ex(self, mi, k, combined):
        p = mi.ptrs.get(k)
        if p is None:
            return None
        if combined in p.by_index:
            return p.by_index[combined]
        return p.default

    def count_is_array(self, mi, k, combined):
        return static_value(self.count_ex(mi, k, combined)) != 1

    def any_count_is_array(self, mi, k):
        p = mi.ptrs.get(k)
        if p is None:
            return False
        if static_value(p.default) != 1:
            return True
        return any(static_value(e) != 1 for e in p.by_index.values())

    def classify(self, mi, indices):
        mods = mi.mods
        k = len(indices)
        cur = mods[k] if k < len(mods) else None
        nxt = mods[k + 1] if k + 1 < len(mods) else None
        comb = self.combined_index(mi, indices)
        following_ptr = any(m[0] == 'ptr' for m in mods[k + 1:])
        if cur is not None and cur[0] == 'arr' and k in mi.dynsize:
            return 'DYNAMIC_ARRAY'
        if cur is not None and cur[0] == 'ptr' and not following_ptr:
            return 'ARRAY_POINTER' if self.count_is_array(mi, k, comb) else 'SINGLE_POINTER'
        if cur is not None and nxt is not None:
            this_arr = (cur[0] == 'ptr' and self.count_is_array(mi, k, comb)) or cur[0] == 'arr'
            next_single = nxt[0] == 'ptr' and not self.any_count_is_array(mi, k + 1)
            if this_arr and next_single:
                return 'POINTER_ARRAY'
        if cur is not None and cur[0] == 'arr' and nxt is None:
            return 'EMBEDDED_ARRAY'
        if cur is None:
            return 'EMBEDDED'
        if cur[0] == 'arr':
            return 'REFERENCE_ARRAY'
        raise GenError('cannot classify %s.%s' % (mi.parent.name, mi.name))

    def slot_offset(self, mi, indices):
        """Byte offset of the element addressed by `indices` (leading arrays)."""
        off = mi.m.offset
        for d, v in enumerate(indices):
            off += v * self.elem_stride(mi, d)
        return off

    def elem_stride(self, mi, depth):
        """Size of one element at array depth `depth`."""
        size = 1
        rest = mi.mods[depth + 1:]
        if any(m[0] == 'ptr' for m in rest):
            n = 1
            for m in rest:
                if m[0] == 'ptr':
                    break
                n *= m[1]
            return PTR * n
        for m in rest:
            size *= m[1]
        return size * self.base_size(mi.base)

    def pointee_size(self, mi, k):
        """Size of what pointer modifier k points at (one element)."""
        rest = mi.mods[k + 1:]
        if any(m[0] == 'ptr' for m in rest):
            n = 1
            for m in rest:
                if m[0] == 'ptr':
                    break
                n *= m[1]
            return PTR * n
        size = self.base_size(mi.base)
        for m in rest:
            size *= m[1]
        return size

    def alloc_align(self, mi, k):
        """Member allocations align to the member's own override, else to a
        pointer when more pointers follow, else to the declared base type
        (a typedef's alignment wins over its struct's; a struct-level
        allocalign applies only to that struct's own pointer loads)."""
        if mi.alloc_align is not None:
            v = static_value(mi.alloc_align)
            return str(v) if v is not None else '(%s as usize)' % self.expr(mi.alloc_align)
        if any(m[0] == 'ptr' for m in mi.mods[k + 1:]):
            return str(PTR)
        return str(self.oat_align_of_base(mi.base))

    def elem_alloc_align(self, mi):
        """Alignment of one pointer-array element's allocation: the struct's
        allocation alignment when the member names the struct itself, the
        declared type's alignment otherwise (a typedef)."""
        if mi.type is not None and mi.base == mi.type.name:
            return self.struct_alloc_align(mi.type)
        return self.oat_align_of_base(mi.base)

    def block_of(self, mi):
        return mi.block

    def in_runtime(self, mi):
        return mi.block is not None and mi.block.kind == 'runtime'

    def in_temp(self, mi):
        return mi.block is not None and mi.block.kind == 'temp'

    # -- member emission (LoadMember_*) ------------------------------------
    def member(self, si, mi, out, ind):
        if mi.condition is not None and static_value(mi.condition) is None:
            out.append('%sif %s {' % (ind, self.cond(mi.condition)))
            self.reference(si, mi, [], out, ind + '    ')
            out.append('%s}' % ind)
        else:
            self.reference(si, mi, [], out, ind)

    def reference(self, si, mi, indices, out, ind):
        lt = self.classify(mi, indices)
        if lt == 'REFERENCE_ARRAY':
            n = mi.mods[len(indices)][1]
            for i in range(n):
                self.reference(si, mi, indices + [i], out, ind)
            return
        blk = self.block_of(mi)
        push = blk is not None and not (blk.kind == 'normal' and blk.default)
        if push:
            out.append('%sw.push(%d)?;' % (ind, blk.index))
        self.load_member(si, mi, indices, lt, out, ind)
        if push:
            out.append('%sw.pop()?;' % ind)

    def load_member(self, si, mi, indices, lt, out, ind):
        k = len(indices)
        off = self.slot_offset(mi, indices)
        comb = self.combined_index(mi, indices)
        slot = 'p.at(%d)' % off
        tname = mi.type.name if mi.type else None
        asset = mi.type is not None and mi.type.asset is not None
        reusable = 'true' if mi.reusable else 'false'
        in_temp = 'true' if self.in_temp(mi) else 'false'

        if mi.is_string:
            if lt == 'SINGLE_POINTER':
                out.append('%sw.xstring(%s)?;' % (ind, slot))
                return
            if lt == 'POINTER_ARRAY':
                if mi.mods[k][0] == 'arr':
                    n = mi.mods[k][1]
                    if k in mi.dyncount:
                        out.append('%s{ let n = w.count(%s)?; w.xstrings(%s, n)?; }' % (ind, self.expr(mi.dyncount[k]), slot))
                    else:
                        out.append('%sw.xstrings(%s, %d)?;' % (ind, slot, n))
                    return
                cnt = self.count_ex(mi, k, comb)
                out.append('%s{' % ind)
                out.append('%s    let n = w.count(%s)?;' % (ind, self.expr(cnt)))
                out.append('%s    w.member_ptr(%s, %s, %s, %s, |w, b| { w.load_at(b, n * 4)?; w.xstrings(b, n) })?;'
                           % (ind, slot, reusable, in_temp, self.alloc_align(mi, k)))
                out.append('%s}' % ind)
                return
            raise GenError('string member %s.%s as %s' % (si.name, mi.name, lt))

        if asset and lt == 'SINGLE_POINTER':
            # LoadMember_Asset (after an optional reuse check).
            if mi.reusable:
                out.append('%sif matches!(w.ptr(%s)?, ZonePtr::Following%s) { lp_%s(w, %s)?; }'
                           % (ind, slot, ' | ZonePtr::Insert' if self.in_temp(mi) else '', tname, slot))
            else:
                out.append('%slp_%s(w, %s)?;' % (ind, tname, slot))
            return

        if lt == 'POINTER_ARRAY':
            fixed = mi.mods[k][0] == 'arr'
            out.append('%s{' % ind)
            if fixed:
                n_expr = self.expr(mi.dyncount[k]) if k in mi.dyncount else str(mi.mods[k][1])
                out.append('%s    let n = w.count(%s)?;' % (ind, n_expr) if k in mi.dyncount else '%s    let n = %s;' % (ind, n_expr))
                out.append('%s    let b = %s;' % (ind, slot))
                self.ptr_array_elems(mi, k + 1, out, ind + '    ')
            else:
                cnt = self.count_ex(mi, k, comb)
                out.append('%s    let n = w.count(%s)?;' % (ind, self.expr(cnt)))
                out.append('%s    w.member_ptr(%s, %s, %s, %s, |w, b| {' % (ind, slot, reusable, in_temp, self.alloc_align(mi, k)))
                out.append('%s        w.load_at(b, n * 4)?;' % ind)
                self.ptr_array_elems(mi, k + 1, out, ind + '        ')
                out.append('%s        Ok(())' % ind)
                out.append('%s    })?;' % ind)
            out.append('%s}' % ind)
            return

        if lt in ('SINGLE_POINTER', 'ARRAY_POINTER'):
            size = self.pointee_size(mi, k)
            walk = mi.type is not None and not mi.type.is_leaf and not self.in_runtime(mi) \
                and not any(m[0] == 'arr' for m in mi.mods[k + 1:])
            out.append('%s{' % ind)
            if lt == 'ARRAY_POINTER':
                out.append('%s    let n = w.count(%s)?;' % (ind, self.expr(self.count_ex(mi, k, comb))))
            else:
                out.append('%s    let n = 1usize;' % ind)
            out.append('%s    w.member_ptr(%s, %s, %s, %s, |w, b| {' % (ind, slot, reusable, in_temp, self.alloc_align(mi, k)))
            if walk and lt == 'SINGLE_POINTER':
                out.append('%s        ld_%s(w, b, true)' % (ind, tname))
            elif walk:
                if dynamic_member(mi.type) is not None:
                    raise GenError('array of dynamic structs %s.%s' % (si.name, mi.name))
                out.append('%s        w.load_at(b, n * %d)?;' % (ind, size))
                out.append('%s        for i in 0..n { ld_%s(w, b.at(i * %d), false)?; }' % (ind, tname, size))
                out.append('%s        Ok(())' % ind)
            else:
                out.append('%s        w.load_at(b, n * %d)' % (ind, size))
            out.append('%s    })?;' % ind)
            out.append('%s}' % ind)
            return

        if lt == 'EMBEDDED':
            apl = after_partial_load(mi)
            if not mi.is_leaf:
                out.append('%sld_%s(w, %s, %s)?;' % (ind, tname, slot, 'true' if apl else 'false'))
            elif apl:
                out.append('%sw.load_at(%s, %d)?;' % (ind, slot, self.elem_size_at(mi, k)))
            return

        if lt == 'EMBEDDED_ARRAY':
            apl = after_partial_load(mi)
            esz = self.elem_stride(mi, k)
            if k in mi.dyncount:
                n = 'w.count(%s)?' % self.expr(mi.dyncount[k])
            else:
                n = str(mi.mods[k][1])
            if not mi.is_leaf:
                out.append('%s{ let n = %s; for i in 0..n { ld_%s(w, %s.at(i * %d), %s)?; } }'
                           % (ind, n, tname, slot, esz, 'true' if apl else 'false'))
            elif apl:
                out.append('%s{ let n = %s; w.load_at(%s, n * %d)?; }' % (ind, n, slot, esz))
            return

        if lt == 'DYNAMIC_ARRAY':
            esz = self.elem_stride(mi, k)
            out.append('%s{' % ind)
            out.append('%s    let n = w.count(%s)?;' % (ind, self.expr(mi.dynsize[k])))
            out.append('%s    w.load_at(%s, n * %d)?;' % (ind, slot, esz))
            if mi.type is not None and not mi.type.is_leaf:
                out.append('%s    for i in 0..n { ld_%s(w, %s.at(i * %d), false)?; }' % (ind, tname, slot, esz))
            out.append('%s}' % ind)
            return

        raise GenError('unhandled %s for %s.%s' % (lt, si.name, mi.name))

    def elem_size_at(self, mi, k):
        size = self.base_size(mi.base)
        for m in mi.mods[k:]:
            size *= m[1]
        return size

    def ptr_array_elems(self, mi, k, out, ind):
        """Elements of a pointer array whose pointers sit at `b` (count `n`)."""
        tname = mi.type.name if mi.type else None
        if mi.type is not None and mi.type.asset is not None:
            out.append('%sfor i in 0..n { lp_%s(w, b.at(i * 4))?; }' % (ind, tname))
            return
        reusable = 'true' if mi.reusable else 'false'
        size = self.pointee_size(mi, k)
        align = self.elem_alloc_align(mi)
        if mi.type is not None and not mi.type.is_leaf:
            out.append('%sfor i in 0..n { w.elem_ptr(b.at(i * 4), %s, %d, |w, e| ld_%s(w, e, true))?; }'
                       % (ind, reusable, align, tname))
        else:
            out.append('%sfor i in 0..n { w.elem_ptr(b.at(i * 4), %s, %d, |w, e| w.load_at(e, %d))?; }'
                       % (ind, reusable, align, size))

    # -- per-structure functions -----------------------------------------
    def struct_fn(self, si):
        if si.ambiguous:
            raise GenError('union %s has more than one member without a condition' % si.name)
        out = []
        dyn = dynamic_member(si)
        out.append("fn ld_%s(w: &mut Walker<'_, '_>, p: Ptr, at_start: bool) -> Result<()> {" % si.name)
        out.append('    w.enter(%d, p)?;' % si.id)
        if not (si.is_union and dyn is not None):
            size = si.rec.size if dyn is None else dyn.m.offset
            out.append('    if at_start { w.load_at(p, %d)?; }' % size)
        else:
            out.append('    let _ = at_start;')
        push = None
        if si.asset is not None:
            push = self.default_normal.index
        elif si.block is not None:
            push = si.block.index
        if push is not None:
            out.append('    w.push(%d)?;' % push)
        if si.is_union:
            used = used_members(si)
            treated = [m for m in si.ordered if needs_treatment(m)]
            first = True
            for mi in treated:
                body = []
                self.reference(si, mi, [], body, '        ')
                cond = mi.condition if mi.condition is not None and static_value(mi.condition) is None else None
                is_last = used and mi is used[-1]
                if first:
                    if cond is not None:
                        out.append('    if %s {' % self.cond(cond))
                    else:
                        if len(treated) > 1:
                            raise GenError('first union member without condition in ' + si.name)
                        out.append('    {')
                    first = False
                elif cond is not None:
                    out.append('    else if %s {' % self.cond(cond))
                elif is_last:
                    out.append('    else {')
                else:
                    raise GenError('middle union member without condition: %s.%s' % (si.name, mi.name))
                out.extend(body)
                out.append('    }')
        else:
            for mi in si.ordered:
                if needs_treatment(mi):
                    self.member(si, mi, out, '    ')
        if push is not None:
            out.append('    w.pop()?;')
        out.append('    w.leave();')
        out.append('    Ok(())')
        out.append('}')
        return out

    def asset_fn(self, si):
        temp = si.block is not None and si.block.kind == 'temp'
        return [
            "fn lp_%s(w: &mut Walker<'_, '_>, slot: Ptr) -> Result<()> {" % si.name,
            '    w.asset_ptr(slot, %d, %s, %d, ld_%s)' % (si.id, 'true' if temp else 'false',
                                                       self.struct_alloc_align(si), si.name),
            '}',
        ]

    def emit(self):
        m = self.model
        infos = list(m.infos.values())
        out = [
            '// @generated by crates/fastfile_t6/gen/t6gen.py from T6 type layouts and zone-load',
            '// rules. Do not edit by hand: change the generator and run it again.',
            '#![allow(non_snake_case, unused_variables, unused_parens, unused_imports, dead_code, clippy::all)]',
            '',
            'use crate::asset_type::AssetType;',
            'use crate::walk::{Kind, Walker};',
            'use crate::zone::{Ptr, Result, ZoneError, ZonePtr};',
            '',
            'pub(crate) const TYPE_COUNT: usize = %d;' % len(infos),
            '',
            'pub(crate) static TYPE_NAMES: [&str; TYPE_COUNT] = [',
        ]
        out += ['    "%s",' % si.name for si in infos]
        out.append('];')
        out.append('')
        out.append('pub(crate) static ASSET_OF_TYPE: [Option<AssetType>; TYPE_COUNT] = [')
        for si in infos:
            a = ASSET_OF_STRUCT.get(si.name) if si.asset else None
            out.append('    %s,' % ('Some(AssetType::%s)' % a if a else 'None'))
        out.append('];')
        out.append('')
        out.append("pub(crate) fn load_xasset(w: &mut Walker<'_, '_>, ty: AssetType, slot: Ptr) -> Result<()> {")
        out.append('    match ty {')
        for variant, struct in ASSET_DISPATCH:
            out.append('        AssetType::%s => lp_%s(w, slot),' % (variant, struct))
        out.append('        other => Err(ZoneError::NoAssetLoader(other)),')
        out.append('    }')
        out.append('}')
        reach = self.reachable()
        for si in infos:
            if si.name not in reach:
                continue
            if si.asset is not None:
                out.append('')
                out += self.asset_fn(si)
            if not si.is_leaf or si.asset is not None:
                out.append('')
                out += self.struct_fn(si)
        return '\n'.join(out) + '\n'

    def reachable(self):
        seen = set()
        todo = [self.model.infos[s] for _, s in ASSET_DISPATCH]
        while todo:
            si = todo.pop()
            if si.name in seen:
                continue
            seen.add(si.name)
            for mi in si.members:
                if mi.type is not None and not ignored(mi):
                    todo.append(mi.type)
        return seen


RUST_KEYWORDS = {
    'as', 'break', 'const', 'continue', 'crate', 'else', 'enum', 'extern', 'false', 'fn', 'for', 'if', 'impl',
    'in', 'let', 'loop', 'match', 'mod', 'move', 'mut', 'pub', 'ref', 'return', 'self', 'Self', 'static',
    'struct', 'super', 'trait', 'true', 'type', 'unsafe', 'use', 'where', 'while', 'async', 'await', 'dyn',
    'abstract', 'become', 'box', 'do', 'final', 'macro', 'override', 'priv', 'typeof', 'unsized', 'virtual',
    'yield', 'try', 'gen',
}


def rust_ident(name):
    return 'r#' + name if name in RUST_KEYWORDS else name


def emit_layout(model):
    """One module per named structure: SIZE, ALIGN and every member's byte
    offset (bit-fields: the storage unit's offset plus _SHIFT and _BITS)."""
    out = [
        '// @generated by crates/fastfile_t6/gen/t6gen.py: T6 structure layouts (32-bit), checked',
        '// against the x86 MSVC compiler by gen/layoutcheck.py. Do not edit by hand.',
        '#![allow(non_snake_case, non_upper_case_globals, dead_code)]',
    ]
    for name in model.h.order:
        rec = model.h.records[name]
        if rec.anon:
            continue
        out.append('')
        out.append('pub mod %s {' % rust_ident(name))
        out.append('    pub const SIZE: usize = %d;' % rec.size)
        out.append('    pub const ALIGN: usize = %d;' % rec.align)
        seen = set()
        for m in rec.members:
            if m.anon or m.name in seen or m.name in ('SIZE', 'ALIGN'):
                continue
            seen.add(m.name)
            out.append('    pub const %s: usize = %d;' % (rust_ident(m.name), m.offset))
            if m.bits is not None:
                out.append('    pub const %s_SHIFT: u32 = %d;' % (m.name, m.bit_shift))
                out.append('    pub const %s_BITS: u32 = %d;' % (m.name, m.bits))
        out.append('}')
    return '\n'.join(out) + '\n'


def main():
    oat, out_rs = sys.argv[1], sys.argv[2]
    check_cpp = sys.argv[3] if len(sys.argv) > 3 else None
    hdir = os.path.join(oat, 'src', 'Common', 'Game', 'T6')
    h = cheader.parse_header(open(os.path.join(hdir, 'T6_Assets.h'), encoding='utf-8').read())
    cheader.parse_header(open(os.path.join(hdir, 'T6.h'), encoding='utf-8').read(), h, enums_only=True)
    model = Model(h)
    cmd = read_commands(os.path.join(oat, 'src', 'ZoneCode', 'Game', 'T6', 'T6_Commands.txt'))
    for st in split_statements(cmd):
        try:
            model.apply(st)
        except (GenError, ExprError, KeyError, IndexError) as e:
            raise GenError('rule %r: %s' % (' '.join(st), e))
    compute_leafs(model)
    fix_unions(model)
    if check_cpp:
        n = layoutcheck.write_check(h, os.path.join(hdir, 'T6_Assets.h'), check_cpp)
        print('layout checks written:', n)
    em = Emitter(model)
    text = em.emit()
    with open(out_rs, 'w', encoding='utf-8', newline='\n') as f:
        f.write(text)
    print('wrote %s: %d lines, %d types' % (out_rs, text.count('\n'), len(model.infos)))
    layout_rs = os.path.join(os.path.dirname(out_rs), 'layout_gen.rs')
    text = emit_layout(model)
    with open(layout_rs, 'w', encoding='utf-8', newline='\n') as f:
        f.write(text)
    print('wrote %s: %d lines' % (layout_rs, text.count('\n')))


if __name__ == '__main__':
    main()
