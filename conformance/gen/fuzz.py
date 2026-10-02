#!/usr/bin/env python3
"""Writes conformance/cases/fuzz.edn: random databases, and random reads of them. A case is a connection through
which random transactions go, right and wrong, and then queries, pulls, entities and index reads of what is left.
Seeded, so the file is the same each time: conformance/gen/fuzz.py [cases] [seed] [out]."""
import random
import sys

N_CASES = int(sys.argv[1]) if len(sys.argv) > 1 else 400
SEED = int(sys.argv[2]) if len(sys.argv) > 2 else 20260101
OUT = sys.argv[3] if len(sys.argv) > 3 else 'conformance/cases/fuzz.edn'
rnd = random.Random(SEED)

SCHEMAS = [
    '{:name {:db/unique :db.unique/identity} :email {:db/unique :db.unique/value} :age {:db/index true} :aka {:db/cardinality :db.cardinality/many} '
    ':friend {:db/valueType :db.type/ref :db/cardinality :db.cardinality/many} :boss {:db/valueType :db.type/ref} '
    ':part {:db/valueType :db.type/ref :db/isComponent true :db/cardinality :db.cardinality/many} :spec {:db/valueType :db.type/ref :db/isComponent true} '
    ':tags {:db/cardinality :db.cardinality/many :db/index true}}',
    '{:name {:db/unique :db.unique/identity} :friend {:db/valueType :db.type/ref :db/cardinality :db.cardinality/many} :boss {:db/valueType :db.type/ref} :aka {:db/cardinality :db.cardinality/many} '
    ':name+age {:db/tupleAttrs [:name :age] :db/index true} :pos {:db/valueType :db.type/tuple :db/tupleTypes [:db.type/long :db.type/long]}}',
    '{:friend {:db/valueType :db.type/ref} :boss {:db/valueType :db.type/ref :db/index true} :aka {:db/cardinality :db.cardinality/many} :tags {:db/cardinality :db.cardinality/many}}',
    'nil',
]
SCALAR_ATTRS = [':name', ':email', ':age', ':score', ':city', ':flag']
MANY_ATTRS = [':aka', ':tags']
REF_ATTRS = [':friend', ':boss', ':part', ':spec']
ALL_ATTRS = SCALAR_ATTRS + MANY_ATTRS + REF_ATTRS
NAMES = ['"Ivan"', '"Petr"', '"Oleg"', '"Anna"', '"Vera"', '"Igor"', '"Olga"', '"Nina"']
EMAILS = ['"a@x"', '"b@x"', '"c@x"', '"d@x"', '"e@x"']
CITIES = ['"Omsk"', '"Perm"', '"Tula"']
TAGS = [':t1', ':t2', ':t3', ':t4', ':a/b']
AKAS = ['"x"', '"y"', '"z"', '"w"', '"long aka"']

def eid(): return str(rnd.randint(1, 8))
def tempid(): return rnd.choice(['-1', '-2', '-3', '"t1"', '"t2"'])
def value(a):
    if a == ':name': return rnd.choice(NAMES)
    if a == ':email': return rnd.choice(EMAILS)
    if a == ':age': return str(rnd.choice([10, 15, 20, 25, 30, 35, 40, 45.5]))
    if a == ':score': return rnd.choice(['1.5', '-2', '0', '100', '3.25'])
    if a == ':city': return rnd.choice(CITIES)
    if a == ':flag': return rnd.choice(['true', 'false'])
    if a == ':aka': return rnd.choice(AKAS)
    if a == ':tags': return rnd.choice(TAGS)
    if a == ':pos': return '[%d %d]' % (rnd.randint(0, 3), rnd.randint(0, 3))
    return ref()
def ref():
    r = rnd.random()
    if r < 0.7: return eid()
    if r < 0.85: return tempid()
    return '[:name %s]' % rnd.choice(NAMES)
def any_eid():
    r = rnd.random()
    if r < 0.6: return eid()
    if r < 0.85: return tempid()
    if r < 0.95: return '[:name %s]' % rnd.choice(NAMES)
    return rnd.choice([':db/current-tx', '[:email "a@x"]', '99', '0', 'nil', '"datomic.tx"'])

def entity_map(depth=0):
    parts = []
    r = rnd.random()
    if r < 0.5: parts.append(':db/id %s' % any_eid())
    for a in rnd.sample(SCALAR_ATTRS, rnd.randint(0, 3)):
        parts.append('%s %s' % (a, value(a)))
    for a in rnd.sample(MANY_ATTRS, rnd.randint(0, 2)):
        vs = [value(a) for _ in range(rnd.randint(0, 3))]
        if rnd.random() < 0.8:
            parts.append('%s %s' % (a, rnd.choice(['[%s]', '#{%s}', '[%s]']) % ' '.join(sorted(set(vs)))))
        else:
            parts.append('%s %s' % (a, value(a)))
    for a in rnd.sample(REF_ATTRS, rnd.randint(0, 2)):
        r = rnd.random()
        if r < 0.5: parts.append('%s %s' % (a, ref()))
        elif r < 0.7: parts.append('%s [%s]' % (a, ' '.join(ref() for _ in range(rnd.randint(0, 3)))))
        elif depth < 2: parts.append('%s %s' % (a, entity_map(depth + 1)))
        else: parts.append('%s %s' % (a, ref()))
    if rnd.random() < 0.15:
        a = rnd.choice(REF_ATTRS)
        parts.append(':_%s %s' % (a[1:], rnd.choice([ref(), '[%s %s]' % (ref(), ref())])))
    if rnd.random() < 0.1: parts.append(':pos %s' % value(':pos'))
    return '{%s}' % ' '.join(parts)

def tx_op():
    r = rnd.random()
    a = rnd.choice(ALL_ATTRS)
    if r < 0.30: return entity_map()
    if r < 0.55: return '[:db/add %s %s %s]' % (any_eid(), a, value(a))
    if r < 0.65: return '[:db/retract %s %s %s]' % (any_eid(), a, value(a))
    if r < 0.70: return '[:db/retract %s %s]' % (any_eid(), a)
    if r < 0.76: return '[%s %s]' % (rnd.choice([':db.fn/retractEntity', ':db/retractEntity']), any_eid())
    if r < 0.82: return '[:db.fn/retractAttribute %s %s]' % (any_eid(), a)
    if r < 0.88:
        a = rnd.choice(SCALAR_ATTRS)
        return '[%s %s %s %s %s]' % (rnd.choice([':db.fn/cas', ':db/cas']), eid(), a, rnd.choice([value(a), 'nil']), value(a))
    if r < 0.92: return '{:db/id %s %s %s}' % (any_eid(), ':_' + rnd.choice(REF_ATTRS)[1:], ref())
    if r < 0.95: return '[:db.fn/call #f %s]' % rnd.choice(['tx-add %s :note "n"' % eid(), 'tx-inc %s :age 1' % eid(), 'tx-nothing', 'tx-count %s :count' % eid(), 'tx-entity %s' % entity_map(2)])
    return rnd.choice(['[:db/add %s :name nil]' % eid(), '[:db/add]', '[:db/unknown 1 :a 1]', 'nil', '5', '[:db/add %s 5 1]' % eid(), '{:db/id %s}' % any_eid(), '[]', '[:db/add %s :pos [1]]' % eid(),
                       '#datascript/Datom [%s :name %s]' % (eid(), rnd.choice(NAMES)), '#datascript/Datom [%s :aka "d" 536870913 false]' % eid(), '{:name %s :age 1 :db/id %s}' % (rnd.choice(NAMES), tempid())])

def tx(): return '[%s]' % ' '.join(tx_op() for _ in range(rnd.randint(1, 5)))

# ---------------------------------------------------------------- reads
DB = '#r db'
def components(index):
    order = {'eavt': 'eav', 'aevt': 'aev', 'avet': 'ave'}[index]
    a = rnd.choice(ALL_ATTRS)
    full = {'e': eid(), 'a': a, 'v': value(a) if not a in REF_ATTRS else eid()}
    return ' '.join(full[c] for c in order[:rnd.randint(0, 3)])

def read_index():
    index = rnd.choice(['eavt', 'aevt', 'avet'])
    op = rnd.choice(['datoms', 'datoms', 'seek-datoms', 'rseek-datoms', 'find-datom'])
    limit = ' :limit %d' % rnd.randint(1, 6) if op in ('seek-datoms', 'rseek-datoms') and rnd.random() < 0.7 else ''
    return '{:op :%s :db %s :index :%s :components [%s]%s}' % (op, DB, index, components(index), limit)

def read_range():
    a = rnd.choice([':age', ':name', ':tags', ':boss', ':email', ':city'])
    return '{:op :index-range :db %s :attr %s :start %s :end %s}' % (DB, a, rnd.choice([value(a), 'nil']), rnd.choice([value(a), 'nil']))

def pattern(depth=0):
    parts = []
    for _ in range(rnd.randint(1, 4)):
        r = rnd.random()
        a = rnd.choice(ALL_ATTRS)
        if r < 0.35: parts.append(a)
        elif r < 0.42: parts.append('*')
        elif r < 0.47: parts.append(':db/id')
        elif r < 0.55: parts.append(':_' + rnd.choice(REF_ATTRS)[1:])
        elif r < 0.63: parts.append('[%s :as %s]' % (a, rnd.choice([':alias', '"s"', ':x/y', '1'])))
        elif r < 0.70: parts.append('[%s :default %s]' % (a, rnd.choice(['0', '"none"', '[]', 'false'])))
        elif r < 0.76: parts.append('[%s :limit %s]' % (rnd.choice(MANY_ATTRS + [':friend', ':part']), rnd.choice(['1', '2', 'nil'])))
        elif r < 0.80: parts.append('[%s :xform %s]' % (a, rnd.choice(['vector', 'str', '#f x-count'])))
        elif r < 0.92 and depth < 2:
            ra = rnd.choice(REF_ATTRS + [':_friend', ':_boss', ':_part'])
            parts.append('{%s %s}' % (ra, rnd.choice([pattern(depth + 1), '...', '1', '2', pattern(depth + 1)])))
        else: parts.append(rnd.choice(['(limit :aka 1)', '(default :city "?")', ':nope', '[:nope :default 1]']))
    return '[%s]' % ' '.join(parts)

def read_pull():
    r = rnd.random()
    if r < 0.75: return '{:op :pull :db %s :pattern %s :eid %s}' % (DB, pattern(), rnd.choice([eid(), eid(), '[:name %s]' % rnd.choice(NAMES), '99']))
    if r < 0.9: return '{:op :pull-many :db %s :pattern %s :eids [%s]}' % (DB, pattern(), ' '.join(eid() for _ in range(rnd.randint(0, 4))))
    return '{:op :pull-visit :db %s :pattern %s :eid %s}' % (DB, pattern(), eid())

def read_entity():
    attrs = rnd.sample(ALL_ATTRS + [':_friend', ':_boss', ':_part', ':_spec', ':db/id', ':nope'], rnd.randint(0, 5))
    return '{:op :entity :db %s :eid %s :attrs [%s]%s}' % (DB, rnd.choice([eid(), eid(), '[:name %s]' % rnd.choice(NAMES)]), ' '.join(attrs), rnd.choice(['', ' :touch true']))

VARS = ['?e', '?e2', '?e3', '?v', '?v2', '?n', '?a', '?x']
class Q:
    def __init__(self):
        self.bound = []
        # how many patterns stand alone, joined to nothing before them: their product is the size of the result
        self.islands = [0]

    def var(self, fresh=0.5):
        if self.bound and rnd.random() > fresh: return rnd.choice(self.bound)
        v = rnd.choice(VARS)
        return v

    def bind(self, *vs):
        for v in vs:
            if v.startswith('?') and v not in self.bound: self.bound.append(v)

    def data_pattern(self):
        a = rnd.choice(ALL_ATTRS)
        e = rnd.choice([self.var(), self.var(), eid(), '_', '[:name %s]' % rnd.choice(NAMES)])
        if (e == '_' or (e.startswith('?') and e not in self.bound)) and self.bound:
            if self.islands[0] >= 1:
                e = rnd.choice(self.bound)
            else:
                self.islands[0] += 1
        r = rnd.random()
        if r < 0.15:
            self.bind(e)
            return '[%s %s]' % (e, a)
        if r < 0.25:
            av, v = self.var(), self.var()
            self.bind(e, av, v)
            return '[%s %s %s]' % (e, av, v)
        v = rnd.choice([self.var(), self.var(), value(a), '_'])
        self.bind(e, v)
        if rnd.random() < 0.08:
            t = rnd.choice([self.var(), '_', '536870913'])
            self.bind(t)
            return '[%s %s %s %s]' % (e, a, v, t)
        return '[%s %s %s]' % (e, a, v)

    def pred(self):
        if not self.bound: return self.data_pattern()
        v = rnd.choice(self.bound)
        r = rnd.random()
        if r < 0.3: return '[(%s %s %s)]' % (rnd.choice(['<', '>', '<=', '>=', '=', '!=', 'not=']), v, rnd.choice([rnd.choice(self.bound), '20', '"Oleg"', ':t2', '3']))
        if r < 0.4: return '[(%s %s)]' % (rnd.choice(['some?', 'nil?', 'true?', 'false?', 'not', 'identity']), v)
        if r < 0.5: return '[(missing? $ %s %s)]' % (rnd.choice(self.bound + [eid()]), rnd.choice(ALL_ATTRS))
        if r < 0.6:
            out = self.var(0.9)
            s = '[(get-else $ %s %s %s) %s]' % (rnd.choice(self.bound + [eid()]), rnd.choice(SCALAR_ATTRS), rnd.choice(['"-"', '0', 'false']), out)
            self.bind(out)
            return s
        if r < 0.68:
            out = self.var(0.9)
            s = '[(get-some $ %s %s %s) [_ %s]]' % (rnd.choice(self.bound + [eid()]), rnd.choice(ALL_ATTRS), rnd.choice(ALL_ATTRS), out)
            self.bind(out)
            return s
        if r < 0.8:
            out = self.var(0.95)
            f = rnd.choice(['(str %s)', '(vector %s 1)', '(identity %s)', '(count [%s])', '(hash-map :k %s)', '(some? %s)', '(str %s "!")', '(ground %s)', '(type %s)', '(pr-str %s)', '(tuple %s %s)'])
            s = '[%s %s]' % (f.replace('%s', v), out)
            self.bind(out)
            return s
        if r < 0.9:
            out = self.var(0.95)
            s = '[(ground %s) %s]' % (rnd.choice(['[1 2 3]', '[:t1 :t2]', '["Ivan" "Oleg"]', '[10 20 30]']), rnd.choice(['[%s ...]' % out, '[%s _ _]' % out]))
            self.bind(out)
            return s
        out = self.var(0.95)
        s = '[(ground [[1 "Ivan"] [2 "Petr"] [3 :t1]]) [[%s %s]]]' % (v, out)
        self.bind(out)
        return s

    def clause(self, depth=0):
        r = rnd.random()
        if r < 0.55 or depth > 1: return self.data_pattern()
        if r < 0.75: return self.pred()
        if r < 0.82:
            inner = Q(); inner.bound = list(self.bound); inner.islands = self.islands
            cs = ' '.join(inner.clause(depth + 1) for _ in range(rnd.randint(1, 2)))
            return '(not %s)' % cs
        if r < 0.87:
            inner = Q(); inner.bound = list(self.bound); inner.islands = self.islands
            cs = ' '.join(inner.clause(depth + 1) for _ in range(rnd.randint(1, 2)))
            vs = rnd.sample(self.bound, min(len(self.bound), rnd.randint(1, 2))) if self.bound else ['?e']
            return '(not-join [%s] %s)' % (' '.join(vs), cs)
        if r < 0.94:
            branches = []
            bound = None
            for _ in range(rnd.randint(1, 3)):
                inner = Q(); inner.bound = list(self.bound); inner.islands = self.islands
                n = rnd.randint(1, 2)
                cs = [inner.clause(depth + 1) for _ in range(n)]
                branches.append(cs[0] if n == 1 else '(and %s)' % ' '.join(cs))
                bound = inner.bound if bound is None else [v for v in bound if v in inner.bound]
            self.bound = bound if rnd.random() < 0.9 else self.bound
            return '(or %s)' % ' '.join(branches)
        if r < 0.98:
            branches = []
            for _ in range(rnd.randint(1, 2)):
                inner = Q(); inner.bound = list(self.bound); inner.islands = self.islands
                n = rnd.randint(1, 2)
                cs = [inner.clause(depth + 1) for _ in range(n)]
                branches.append(cs[0] if n == 1 else '(and %s)' % ' '.join(cs))
            vs = rnd.sample(VARS, rnd.randint(1, 2))
            self.bind(*vs)
            return '(or-join [%s] %s)' % (' '.join(vs), ' '.join(branches))
        rule = rnd.choice(['(friends %s %s)', '(boss-of %s %s)', '(named %s %s)', '(linked %s %s)'])
        a, b = self.var(), self.var()
        self.bind(a, b)
        return rule % (a, b)

RULES = ('[[(friends ?a ?b) [?a :friend ?b]] [(friends ?a ?b) [?b :friend ?a]] [(boss-of ?a ?b) [?b :boss ?a]] [(boss-of ?a ?b) [?b :boss ?c] (boss-of ?a ?c)] '
         '[(named ?e ?n) [?e :name ?n]] [(linked ?a ?b) (or [?a :friend ?b] [?a :boss ?b] [?a :part ?b])]]')

def read_query():
    qq = Q()
    ins, inputs = ['$'], [DB]
    r = rnd.random()
    if r < 0.12:
        ins.append('?x'); inputs.append(rnd.choice([eid(), '"Ivan"', ':t1', '20', '[:name "Ivan"]', 'nil'])); qq.bind('?x')
    elif r < 0.2:
        ins.append('[?x ...]'); inputs.append(rnd.choice(['[1 2 3]', '["Ivan" "Oleg" "Zed"]', '[]', '[[:name "Ivan"] 2 [:name "Petr"]]', '#{:t1 :t2}'])); qq.bind('?x')
    elif r < 0.26:
        ins.append('[[?x ?n]]'); inputs.append(rnd.choice(['[[1 "Ivan"] [2 "Petr"] [3 "Oleg"]]', '[]', '{1 "Ivan", 2 "Oleg"}'])); qq.bind('?x', '?n')
    elif r < 0.3:
        ins.append('[?x ?n]'); inputs.append(rnd.choice(['[1 "Ivan"]', '[2 :t1 :extra]'])); qq.bind('?x', '?n')
    clauses = [qq.clause() for _ in range(rnd.randint(1, 4))]
    uses_rules = any(c.startswith(('(friends', '(boss-of', '(named', '(linked')) or ' (friends' in c or ' (boss-of' in c or ' (named' in c or ' (linked' in c for c in clauses)
    if uses_rules or rnd.random() < 0.05:
        ins.append('%'); inputs.append(RULES)
    bound = qq.bound or ['?e']
    find_vars = rnd.sample(bound, min(len(bound), rnd.randint(1, 3)))
    if rnd.random() < 0.06: find_vars.append(rnd.choice(VARS))
    r = rnd.random()
    extra = ''
    if r < 0.55: find = ' '.join(find_vars)
    elif r < 0.63: find = '[%s ...]' % find_vars[0]
    elif r < 0.69: find = '%s .' % find_vars[0]
    elif r < 0.75: find = '[%s]' % ' '.join(find_vars)
    elif r < 0.87:
        agg = rnd.choice(['count', 'count-distinct', 'distinct', 'min', 'max', 'sum', 'avg', 'median', 'min 2', 'max 2'])
        if agg in ('sum', 'avg', 'median', 'min', 'max', 'min 2', 'max 2') and rnd.random() < 0.8:
            # over ages, which are numbers
            clauses.append('[%s :age ?num]' % rnd.choice(bound + ['_']))
            target = '?num'
        else:
            target = find_vars[-1]
        keep = find_vars[:-1] if len(find_vars) > 1 else []
        find = ' '.join(keep + ['(%s %s)' % (agg, target)])
        if rnd.random() < 0.3 and bound: extra = ' :with %s' % rnd.choice(bound)
    elif r < 0.94:
        find = ' '.join(find_vars[:-1] + ['(pull %s %s)' % (find_vars[-1], pattern())])
    else:
        find = ' '.join(find_vars)
        extra = ' %s %s' % (rnd.choice([':keys', ':strs', ':syms']), ' '.join('k%d' % i for i in range(len(find_vars))))
    if extra == '' and rnd.random() < 0.05 and bound: extra = ' :with %s' % rnd.choice(bound)
    in_part = '' if ins == ['$'] and rnd.random() < 0.8 else ' :in %s' % ' '.join(ins)
    return '{:op :q :query [:find %s%s%s :where %s] :inputs [%s]}' % (find, extra, in_part, ' '.join(clauses), ' '.join(inputs))

lines = []
for c in range(N_CASES):
    schema = rnd.choice(SCHEMAS) if rnd.random() < 0.85 else SCHEMAS[0]
    steps = ['{:op :conn :schema %s :txs [%s] :as db}' % (schema, ' '.join(tx() for _ in range(rnd.randint(2, 6))))]
    if rnd.random() < 0.15:
        steps.append('{:op :filter :db #r db :pred #f %s :as db}' % rnd.choice(['f-even-e', 'f-not-name', 'f-all', 'f-has-name']))
    if rnd.random() < 0.15:
        steps.append('{:op :serializable :db #r db :as db}')
    for _ in range(rnd.randint(8, 16)):
        r = rnd.random()
        if r < 0.45: steps.append(read_query())
        elif r < 0.65: steps.append(read_pull())
        elif r < 0.78: steps.append(read_entity())
        elif r < 0.93: steps.append(read_index())
        else: steps.append(read_range())
    lines.append('[' + ' '.join(steps) + ']')

with open(OUT, 'w') as f:
    f.write('; Random databases and random reads of them. Written by conformance/gen/fuzz.py %d %d.\n' % (N_CASES, SEED))
    f.write('\n'.join(lines) + '\n')
print('%d cases' % len(lines))
