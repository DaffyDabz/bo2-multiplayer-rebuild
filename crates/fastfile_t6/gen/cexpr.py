"""bo2zm: C-precedence expression parsing for the T6 generator.

AST nodes:
  ('num', int)
  ('name', 'A::b::c', [index ASTs])     a constant, or a field path
  ('un', op, a)
  ('bin', op, a, b)
  ('tern', c, a, b)
"""

BIN_PREC = [
    ('||',),
    ('&&',),
    ('|',),
    ('^',),
    ('&',),
    ('==', '!='),
    ('<', '<=', '>', '>='),
    ('<<', '>>'),
    ('+', '-'),
    ('*', '/', '%'),
]


class ExprError(Exception):
    pass


class _P:
    def __init__(self, toks):
        self.t = toks
        self.i = 0

    def peek(self):
        return self.t[self.i] if self.i < len(self.t) else None

    def next(self):
        tok = self.t[self.i]
        self.i += 1
        return tok

    def expect(self, v):
        tok = self.next()
        if tok != v:
            raise ExprError('expected %r got %r in %r' % (v, tok, ' '.join(self.t)))

    def ternary(self):
        c = self.binary(0)
        if self.peek() == '?':
            self.next()
            a = self.ternary()
            self.expect(':')
            b = self.ternary()
            return ('tern', c, a, b)
        return c

    def binary(self, level):
        if level == len(BIN_PREC):
            return self.unary()
        left = self.binary(level + 1)
        while self.peek() in BIN_PREC[level]:
            op = self.next()
            right = self.binary(level + 1)
            left = ('bin', op, left, right)
        return left

    def unary(self):
        p = self.peek()
        if p in ('!', '~', '-', '+'):
            self.next()
            return ('un', p, self.unary())
        return self.primary()

    def primary(self):
        p = self.next()
        if p == '(':
            e = self.ternary()
            self.expect(')')
            return e
        if p[0].isdigit():
            s = p.rstrip('uUlL')
            return ('num', int(s, 16) if s.lower().startswith('0x') else int(s))
        if p[0].isalpha() or p[0] == '_':
            name = p
            while self.peek() == '::':
                self.next()
                name += '::' + self.next()
            idx = []
            while self.peek() == '[':
                self.next()
                idx.append(self.ternary())
                self.expect(']')
            return ('name', name, idx)
        raise ExprError('unexpected %r in %r' % (p, ' '.join(self.t)))


def parse_expr(toks):
    toks = [t[1] if isinstance(t, tuple) else t for t in toks]
    p = _P(toks)
    e = p.ternary()
    if p.peek() is not None:
        raise ExprError('trailing %r in %r' % (p.peek(), ' '.join(toks)))
    return e


def _c_div(a, b):
    q = abs(a) // abs(b)
    return q if (a >= 0) == (b >= 0) else -q


def apply_bin(op, a, b):
    if op == '+': return a + b
    if op == '-': return a - b
    if op == '*': return a * b
    if op == '/': return _c_div(a, b)
    if op == '%': return a - _c_div(a, b) * b
    if op == '<<': return a << b
    if op == '>>': return a >> b
    if op == '&': return a & b
    if op == '|': return a | b
    if op == '^': return a ^ b
    if op == '==': return int(a == b)
    if op == '!=': return int(a != b)
    if op == '<': return int(a < b)
    if op == '<=': return int(a <= b)
    if op == '>': return int(a > b)
    if op == '>=': return int(a >= b)
    if op == '&&': return int(bool(a) and bool(b))
    if op == '||': return int(bool(a) or bool(b))
    raise ExprError('op ' + op)


def eval_static(ast, constants):
    """Evaluate with names resolved only through `constants`; raises KeyError
    when the expression depends on a field."""
    k = ast[0]
    if k == 'num':
        return ast[1]
    if k == 'name':
        if ast[2]:
            raise KeyError(ast[1])
        return constants[ast[1]]
    if k == 'un':
        v = eval_static(ast[2], constants)
        return {'!': int(not v), '~': ~v, '-': -v, '+': v}[ast[1]]
    if k == 'bin':
        return apply_bin(ast[1], eval_static(ast[2], constants), eval_static(ast[3], constants))
    if k == 'tern':
        return eval_static(ast[2] if eval_static(ast[1], constants) else ast[3], constants)
    raise ExprError(repr(ast))
