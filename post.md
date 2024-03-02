**What's an `Expr`?**

It is an expression over scalar values, like numbers.

**What's the definition of `Expr` we've been using so far?**

It's this:

```rust
enum Expr {
    ColRef { id: usize },
    Int { val: i64 },
    Eq { left: Box<Expr>, right: Box<Expr> },
}
```

**What's a "free variable?"**

It is a column in an expression that is not bound.

**You're kicking the can down the road: what is a "bound variable," then?**

It is a column that is given its values within an expression and is not a parameter that comes from some other context.

**Can you give me an example?**

In the JavaScript expression,

```javascript
function (a) {
    return a + b;
}
```

`a` is bound, because it doesn't refer to anything outside of the scope of the expression itself, but `b` is free.

**If `expr` is an `Expr`, what is `expr.free()`?**

It is the set of free variables in `expr`.

**Show me `Expr::free`.**

It's 

```rust
fn free(&self) -> HashSet<usize> {
    match self {
        Expr::ColRef { id } => {
            let mut set = HashSet::new();
            set.insert(*id);
            set
        }
        Expr::Int { .. } => HashSet::new(),
        Expr::Eq { left, right } => {
            let mut set = left.free();
            set.extend(right.free());
            set
        }
    }
}
```

**It doesn't seem like there's any such thing as a "bound column."**

Well, not in this simplified example. Work with me here.

**Fine. What's a `RelExpr`?**

It's an expression over relational expressions, like scans, filters, and joins.

**What's the definition of `RelExpr` we've been using so far?**

```rust
enum RelExpr {
    Scan {
        table_name: String,
        column_names: Vec<usize>,
    },
    Select {
        src: Box<RelExpr>,
        predicates: Vec<Expr>,
    },
    Join {
        left: Box<RelExpr>,
        right: Box<RelExpr>,
        predicates: Vec<Expr>,
    },
    Project {
        src: Box<RelExpr>,
        cols: HashSet<usize>,
    },
}
```

```rust
impl RelExpr {
    fn scan(table_name: String, column_names: Vec<usize>) -> Self {
        RelExpr::Scan {
            table_name,
            column_names,
        }
    }

    fn select(self, mut predicates: Vec<Expr>) -> Self {
        RelExpr::Select {
            src: Box::new(self),
            predicates,
        }
    }

    fn join(self, other: Self, mut predicates: Vec<Expr>) -> Self {
        RelExpr::Join {
            left: Box::new(self),
            right: Box::new(other),
            predicates,
        }
    }

    fn project(self, cols: HashSet<usize>) -> Self {
        RelExpr::Project {
            src: Box::new(self),
            cols,
        }
    }
}
```

**What is the *attribute set* of a relational expression?**

It is the set of columns that each row in the evaluated expression has.

**If `rel` is a `RelExpr`, what's `rel.att()`?**

It is the attribute set of the `RelExpr`.

**Show me its implementation.**

```rust
fn att(&self) -> HashSet<usize> {
    match self {
        RelExpr::Scan { column_names, .. } => column_names.iter().cloned().collect(),
        RelExpr::Select { src, .. } => src.att(),
        RelExpr::Join { left, right, .. } => {
            let mut set = left.att();
            set.extend(right.att());
            set
        }
        RelExpr::Project { src, cols } => {
            let mut set = src.att();
            set.retain(|id| cols.contains(id));
            set
        }
    }
}
```

**Are "free variables" and "attribute sets" similar?**

They're both sets of columns that we derive from an expression.

**How are they different?**

Well, "free variables" represent the sets of columns we *need*, and "attribute sets* represent the sets of columns we *have*.

**What does it mean for an `Expr` to be "bound by" a `RelExpr`?**

An `Expr` is "bound by" a `RelExpr` if all of its free variables are part of the `RelExpr`'s attribute set.

**Why is that important?**

When we attempt to evaluate the scalar expression, we do so in the context of a row. We can only evaluate a scalar expression if the row we have has values for all of the columns we reference.

**What's a join?**

It's the cross product of two relations, filtered according to some predicates.

**Can you show me a join?**

```rust
let left = RelExpr::scan("a".into(), vec![0, 1]);
let right = RelExpr::scan("x".into(), vec![2, 3]);

let join = left.join(
    right,
    vec![
        Expr::col_ref(0).eq(Expr::col_ref(2)),
        Expr::col_ref(1).eq(Expr::int(100)),
    ],
);

println!("{:#?}", join);
```

```
-> join(@0=@2)
  -> select(@1=100)
    -> scan("a", [0, 1])
  -> scan("x", [2, 3])
```

**Explain what this join does.**

It looks at every pair of rows from `a` and `x`,
and emits the ones that satisfy both `a.0=x.2` and `a.1=100`.

**How do those predicates differ?**

The first one, `a.0=x.2`, is only bound by both inputs to the join, but the second one, `a.1=100` is bound by only `a`.

**What does that mean?**

We don't need the values from `x` to evaluate if a row satisfies `a.1=100`.

**Does that suggest anything we could do to this query?**

We could execute that predicate before doing the join. We could write it like this:

```rust
-> join(@0=@2)
  -> select(@1=100)
    -> scan("a", [0, 1])
  -> scan("x", [2, 3])
let join = left
    .select(vec![Expr::col_ref(1).eq(Expr::int(100))])
    .join(right, vec![Expr::col_ref(0).eq(Expr::col_ref(2))]);
```

```rust
Join {
    left: Select {
        src: Scan {
            table_name: "a",
            column_names: [
                0,
                1,
            ],
        },
        predicates: [
            Eq {
                left: ColRef {
                    id: 1,
                },
                right: Int {
                    val: 100,
                },
            },
        ],
    },
    right: Scan {
        table_name: "x",
        column_names: [
            2,
            3,
        ],
    },
    predicates: [
        Eq {
            left: ColRef {
                id: 0,
            },
            right: ColRef {
                id: 2,
            },
        },
    ],
}
```

**Can we automate this process?**

Probably.

**How might we do that?**

I guess we'd have to check if each predicate is bound by either side of the join alone.

**Write `bound_by`.**

```rust
impl Expr {
    fn bound_by(&self, rel: &RelExpr) -> bool {
        self.free().is_subset(&rel.att())
    }
}
```

**How can we use this?**

We can intercept calls to `join`:

```rust
for i in 0..predicates.len() {
    if predicates[i].bound_by(&self) {
        // We can push this predicate down.
        let predicate = predicates.swap_remove(i);
        return self.select(vec![predicate]).join(other, predicates);
    }

    if predicates[i].bound_by(&other) {
        // We can push this predicate down.
        let predicate = predicates.swap_remove(i);
        return self.join(other.select(vec![predicate]), predicates);
    }
}
```

**Now show me a query.**

```rust
let left = RelExpr::scan("a".into(), vec![0, 1]);
let right = RelExpr::scan("x".into(), vec![2, 3]);

let join = left.join(
    right,
    vec![
        Expr::col_ref(0).eq(Expr::col_ref(2)),
        Expr::col_ref(1).eq(Expr::int(100)),
        Expr::col_ref(3).eq(Expr::int(100)),
    ],
);
```

It looks like it works.

```rust
Join {
    left: Select {
        src: Scan {
            table_name: "a",
            column_names: [
                0,
                1,
            ],
        },
        predicates: [
            Eq {
                left: ColRef {
                    id: 1,
                },
                right: Int {
                    val: 100,
                },
            },
        ],
    },
    right: Select {
        src: Scan {
            table_name: "x",
            column_names: [
                2,
                3,
            ],
        },
        predicates: [
            Eq {
                left: ColRef {
                    id: 3,
                },
                right: Int {
                    val: 100,
                },
            },
        ],
    },
    predicates: [
        Eq {
            left: ColRef {
                id: 0,
            },
            right: ColRef {
                id: 2,
            },
        },
    ],
}
```

**Nice. I see a couple problems with this, though. What about this?**

```rust
let join = left.join(
    right,
    vec![
        Expr::col_ref(0).eq(Expr::int(100)),
        Expr::col_ref(1).eq(Expr::int(200)),
    ],
);
```

Let me see.

```rust
Join {
    left: Select {
        src: Select {
            src: Scan {
                table_name: "a",
                column_names: [
                    0,
                    1,
                ],
            },
            predicates: [
                Eq {
                    left: ColRef {
                        id: 0,
                    },
                    right: Int {
                        val: 100,
                    },
                },
            ],
        },
        predicates: [
            Eq {
                left: ColRef {
                    id: 1,
                },
                right: Int {
                    val: 200,
                },
            },
        ],
    },
    right: Scan {
        table_name: "x",
        column_names: [
            2,
            3,
        ],
    },
    predicates: [],
}
```

Oh. Kind of ugly. We need to collapse those `Select`s. We can intercept calls to `select` too.

```rust
fn select(self, mut predicates: Vec<Expr>) -> Self {
    if let RelExpr::Select {
        src,
        predicates: mut preds,
    } = self
    {
        preds.append(&mut predicates);
        return src.select(preds);
    }

    RelExpr::Select {
        src: Box::new(self),
        predicates,
    }
}
```

```rust
Join {
    left: Select {
        src: Scan {
            table_name: "a",
            column_names: [
                0,
                1,
            ],
        },
        predicates: [
            Eq {
                left: ColRef {
                    id: 0,
                },
                right: Int {
                    val: 100,
                },
            },
            Eq {
                left: ColRef {
                    id: 1,
                },
                right: Int {
                    val: 200,
                },
            },
        ],
    },
    right: Scan {
        table_name: "x",
        column_names: [
            2,
            3,
        ],
    },
    predicates: [],
}
```

**Much better.**

Apologies to Friedman and Felleisen.