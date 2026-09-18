// ezez — ภาษาเล็ก ๆ ในไฟล์เดียว
// node ezez.js prog.ez   รันไฟล์
// node ezez.js           รัน self-check

const RE = /\/\/[^\n]*|\s+|\d+(?:\.\d+)?|"[^"]*"|[A-Za-z_]\w*|[=!<>]=|&&|\|\||[-+*/%<>=!(){},;]/y;
const KEYWORDS = new Set(['let', 'if', 'else', 'while', 'fn', 'return', 'true', 'false', 'null']);
const PREC = { '||': 1, '&&': 2, '==': 3, '!=': 3, '<': 3, '>': 3, '<=': 3, '>=': 3, '+': 4, '-': 4, '*': 5, '/': 5, '%': 5 };
const OPS = {
  '+': (a, b) => a + b, '-': (a, b) => a - b, '*': (a, b) => a * b, '/': (a, b) => a / b, '%': (a, b) => a % b,
  '<': (a, b) => a < b, '>': (a, b) => a > b, '<=': (a, b) => a <= b, '>=': (a, b) => a >= b,
  '==': (a, b) => a === b, '!=': (a, b) => a !== b,
};

export function lex(src) {
  const out = [];
  RE.lastIndex = 0;
  while (RE.lastIndex < src.length) {
    const start = RE.lastIndex;
    const m = RE.exec(src);
    if (!m) throw new Error(`ezez: อักขระไม่รู้จักที่ ${start}: ${JSON.stringify(src[start])}`);
    if (!/^\s|^\/\//.test(m[0])) out.push(m[0]);
  }
  return out;
}

export function parse(tokens) {
  let i = 0;
  const peek = () => tokens[i];
  const next = () => tokens[i++];
  const eat = (t) => {
    if (tokens[i] !== t) throw new Error(`ezez: ต้องการ '${t}' แต่เจอ '${tokens[i] ?? 'EOF'}'`);
    return tokens[i++];
  };
  const semi = () => { if (peek() === ';') next(); };

  function ident() {
    const t = next();
    if (!/^[A-Za-z_]\w*$/.test(t ?? '') || KEYWORDS.has(t)) throw new Error(`ezez: ต้องการชื่อตัวแปร แต่เจอ '${t ?? 'EOF'}'`);
    return t;
  }

  function params() {
    eat('(');
    const names = [];
    while (peek() !== ')') {
      names.push(ident());
      if (peek() === ',') next();
    }
    eat(')');
    return names;
  }

  function block() {
    eat('{');
    const body = [];
    while (peek() !== '}') {
      if (peek() === undefined) throw new Error("ezez: ขาด '}'");
      body.push(statement());
    }
    eat('}');
    return body;
  }

  function statement() {
    if (peek() === 'let') { next(); const name = ident(); eat('='); const value = expr(); semi(); return { k: 'let', name, value }; }
    if (peek() === 'fn') { next(); const name = ident(); return { k: 'let', name, value: { k: 'fn', params: params(), body: block() } }; }
    if (peek() === 'return') { next(); const value = peek() === ';' || peek() === '}' ? null : expr(); semi(); return { k: 'return', value }; }
    if (peek() === 'while') { next(); eat('('); const cond = expr(); eat(')'); return { k: 'while', cond, body: block() }; }
    if (peek() === 'if') {
      next(); eat('('); const cond = expr(); eat(')');
      const then = block();
      let alt = null;
      if (peek() === 'else') { next(); alt = peek() === 'if' ? [statement()] : block(); }
      return { k: 'if', cond, then, alt };
    }
    const e = expr(); semi(); return { k: 'expr', e };
  }

  function expr() {
    const left = binary(0);
    if (peek() === '=') {
      next();
      if (left.k !== 'var') throw new Error('ezez: ฝั่งซ้ายของ = ต้องเป็นตัวแปร');
      return { k: 'assign', name: left.name, value: expr() };
    }
    return left;
  }

  function binary(min) {
    let left = unary();
    for (;;) {
      const op = peek(), p = PREC[op];
      if (p === undefined || p < min) return left;
      next();
      left = { k: 'bin', op, left, right: binary(p + 1) };
    }
  }

  function unary() {
    if (peek() === '-' || peek() === '!') return { k: 'un', op: next(), operand: unary() };
    let e = primary();
    while (peek() === '(') {
      next();
      const args = [];
      while (peek() !== ')') { args.push(expr()); if (peek() === ',') next(); }
      eat(')');
      e = { k: 'call', fn: e, args };
    }
    return e;
  }

  function primary() {
    const t = next();
    if (t === undefined) throw new Error('ezez: โค้ดจบกลางคัน');
    if (t === '(') { const e = expr(); eat(')'); return e; }
    if (t === 'fn') return { k: 'fn', params: params(), body: block() };
    if (t === 'true') return { k: 'lit', v: true };
    if (t === 'false') return { k: 'lit', v: false };
    if (t === 'null') return { k: 'lit', v: null };
    if (/^\d/.test(t)) return { k: 'lit', v: Number(t) };
    if (t[0] === '"') return { k: 'lit', v: t.slice(1, -1) };
    if (/^[A-Za-z_]/.test(t) && !KEYWORDS.has(t)) return { k: 'var', name: t };
    throw new Error(`ezez: ไม่รู้จัก '${t}'`);
  }

  const prog = [];
  while (i < tokens.length) prog.push(statement());
  return prog;
}

class Env {
  constructor(parent) { this.vars = new Map(); this.parent = parent; }
  get(n) {
    for (let e = this; e; e = e.parent) if (e.vars.has(n)) return e.vars.get(n);
    throw new Error(`ezez: ไม่รู้จักตัวแปร '${n}'`);
  }
  set(n, v) {
    for (let e = this; e; e = e.parent) if (e.vars.has(n)) { e.vars.set(n, v); return v; }
    throw new Error(`ezez: ไม่รู้จักตัวแปร '${n}'`);
  }
}

class Return { constructor(value) { this.value = value; } }

function exec(stmts, env) {
  let last = null;
  for (const s of stmts) {
    if (s.k === 'let') { env.vars.set(s.name, evaluate(s.value, env)); last = null; }
    else if (s.k === 'return') throw new Return(s.value ? evaluate(s.value, env) : null);
    else if (s.k === 'if') {
      if (truthy(evaluate(s.cond, env))) last = exec(s.then, new Env(env));
      else if (s.alt) last = exec(s.alt, new Env(env));
      else last = null;
    }
    else if (s.k === 'while') { while (truthy(evaluate(s.cond, env))) exec(s.body, new Env(env)); last = null; }
    else last = evaluate(s.e, env);
  }
  return last;
}

const truthy = (v) => v !== false && v !== null && v !== 0 && v !== '';

function evaluate(n, env) {
  switch (n.k) {
    case 'lit': return n.v;
    case 'var': return env.get(n.name);
    case 'assign': return env.set(n.name, evaluate(n.value, env));
    case 'fn': return { params: n.params, body: n.body, env };
    case 'un': {
      const v = evaluate(n.operand, env);
      return n.op === '-' ? -v : !truthy(v);
    }
    case 'bin': {
      if (n.op === '&&') return truthy(evaluate(n.left, env)) ? evaluate(n.right, env) : false;
      if (n.op === '||') { const l = evaluate(n.left, env); return truthy(l) ? l : evaluate(n.right, env); }
      return OPS[n.op](evaluate(n.left, env), evaluate(n.right, env));
    }
    case 'call': {
      const fn = evaluate(n.fn, env);
      const args = n.args.map((a) => evaluate(a, env));
      if (typeof fn === 'function') return fn(...args);
      if (!fn || !fn.body) throw new Error('ezez: เรียกอะไรที่ไม่ใช่ฟังก์ชัน');
      if (args.length !== fn.params.length) throw new Error(`ezez: ฟังก์ชันรับ ${fn.params.length} อาร์กิวเมนต์ แต่ได้ ${args.length}`);
      const local = new Env(fn.env);
      fn.params.forEach((p, i) => local.vars.set(p, args[i]));
      try { exec(fn.body, local); } catch (e) {
        if (e instanceof Return) return e.value;
        throw e;
      }
      return null;
    }
  }
}

export function run(src, globals = {}) {
  const env = new Env(null);
  env.vars.set('print', (...a) => { console.log(...a); return null; });
  env.vars.set('str', (v) => String(v));
  env.vars.set('num', (v) => Number(v));
  for (const [k, v] of Object.entries(globals)) env.vars.set(k, v);
  return exec(parse(lex(src)), env);
}

async function demo() {
  const { strictEqual, throws } = await import('node:assert');
  strictEqual(run('1 + 2 * 3'), 7);
  strictEqual(run('(1 + 2) * 3'), 9);
  strictEqual(run('10 - 3 - 4'), 3);                      // left-assoc
  strictEqual(run('1 + 2 < 4 == true'), true);            // ลำดับ: เลข > เทียบ > ==
  strictEqual(run('"a" + "b"'), 'ab');
  strictEqual(run('!false && -2 < 0'), true);
  strictEqual(run('let x = 1; x = x + 41; x'), 42);
  strictEqual(run('let s = 0; let i = 1; while (i <= 100) { s = s + i; i = i + 1 } s'), 5050);
  strictEqual(run('fn fib(n) { if (n < 2) { return n } return fib(n-1) + fib(n-2) } fib(15)'), 610);
  strictEqual(run('fn mk(a) { return fn(b) { return a + b } } mk(3)(4)'), 7);   // closure
  strictEqual(run('fn c() { let n = 0; return fn() { n = n + 1; return n } } let up = c(); up(); up()'), 2);
  strictEqual(run('if (0) { 1 } else if ("") { 2 } else { 3 }'), 3);            // 0 และ "" เป็นเท็จ
  strictEqual(run('fn f() { } f()'), null);                                     // ไม่ return = null
  strictEqual(run('// comment\n1 // อีกอัน\n'), 1);
  strictEqual(run('false || "fallback"'), 'fallback');
  strictEqual(run('str(1 + 1) + "!"'), '2!');
  strictEqual(run('double(21)', { double: (n) => n * 2 }), 42);
  throws(() => run('nope'), /ไม่รู้จักตัวแปร/);
  throws(() => run('fn f(a) { } f()'), /1 อาร์กิวเมนต์ แต่ได้ 0/);
  throws(() => run('1 + '), /จบกลางคัน/);
  throws(() => run('let x = 1 @ 2'), /อักขระไม่รู้จัก/);
  throws(() => run('if (1) { 2'), /ขาด/);
}

const { pathToFileURL } = await import('node:url');
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const file = process.argv[2];
  if (file) {
    const { readFileSync } = await import('node:fs');
    run(readFileSync(file, 'utf8'));
  } else {
    await demo();
    console.log('ezez: self-check ok');
  }
}
