use std::{cell::RefCell, collections::HashSet, rc::Rc};

#[derive(Debug, Clone)]
struct State {
    next_id: Rc<RefCell<usize>>,
}

impl State {
    fn new() -> Self {
        State {
            next_id: Rc::new(RefCell::new(0)),
        }
    }

    fn next(&self) -> usize {
        let id = *self.next_id.borrow();
        *self.next_id.borrow_mut() += 1;
        id
    }
}

#[derive(Debug, Clone)]
enum Expr {
    ColRef { id: usize },
    Int { val: i64 },
    Eq { left: Box<Expr>, right: Box<Expr> },
    Plus { left: Box<Expr>, right: Box<Expr> },
    Subquery { expr: Box<RelExpr> },
}

impl Expr {
    fn col_ref(id: usize) -> Self {
        Expr::ColRef { id }
    }

    fn int(val: i64) -> Self {
        Expr::Int { val }
    }

    fn eq(self, other: Self) -> Self {
        Expr::Eq {
            left: Box::new(self),
            right: Box::new(other),
        }
    }

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
            Expr::Plus { left, right } => {
                let mut set = left.free();
                set.extend(right.free());
                set
            }
            Expr::Subquery { expr } => expr.free(),
        }
    }

    fn bound_by(&self, rel: &RelExpr) -> bool {
        self.free().is_subset(&rel.att())
    }

    fn has_subquery(&self) -> bool {
        match self {
            Expr::ColRef { .. } => false,
            Expr::Int { .. } => false,
            Expr::Eq { left, right } => left.has_subquery() || right.has_subquery(),
            Expr::Plus { left, right } => left.has_subquery() || right.has_subquery(),
            Expr::Subquery { .. } => true,
        }
    }

    fn print(&self, indent: usize, out: &mut String) {
        match self {
            Expr::ColRef { id } => {
                out.push_str(&format!("@{}", id));
            }
            Expr::Int { val } => {
                out.push_str(&format!("{}", val));
            }
            Expr::Eq { left, right } => {
                left.print(indent, out);
                out.push('=');
                right.print(indent, out);
            }
            Expr::Plus { left, right } => {
                left.print(indent, out);
                out.push('+');
                right.print(indent, out);
            }
            Expr::Subquery { expr } => {
                out.push_str("λ.(\n");
                expr.print(indent + 6, out);
                out.push_str(&format!("{})", " ".repeat(indent + 4)));
            }
        }
    }

    fn plus(self, other: Self) -> Self {
        Expr::Plus {
            left: Box::new(self),
            right: Box::new(other),
        }
    }
}

#[derive(Debug, Clone)]
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
    Map {
        input: Box<RelExpr>,
        exprs: Vec<(usize, Expr)>,
    },
    Project {
        src: Box<RelExpr>,
        cols: HashSet<usize>,
    },
    Apply {
        input: Box<RelExpr>,
        func: Box<RelExpr>,
    },
    Unit,
}

impl RelExpr {
    fn scan(table_name: String, column_names: Vec<usize>) -> Self {
        RelExpr::Scan {
            table_name,
            column_names,
        }
    }

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

    fn join(self, other: Self, mut predicates: Vec<Expr>) -> Self {
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

        RelExpr::Join {
            left: Box::new(self),
            right: Box::new(other),
            predicates,
        }
    }

    fn has_subquery(&self) -> bool {
        match self {
            RelExpr::Scan { .. } => false,
            RelExpr::Select { src, .. } => src.has_subquery(),
            RelExpr::Join { left, right, .. } => left.has_subquery() || right.has_subquery(),
            RelExpr::Map { input, exprs } => {
                if input.has_subquery() {
                    return true;
                }

                for (_, expr) in exprs {
                    if expr.has_subquery() {
                        return true;
                    }
                }

                false
            }
            RelExpr::Project { src, .. } => src.has_subquery(),
            // TODO: wrong
            RelExpr::Apply { input, func } => input.has_subquery() || func.has_subquery(),
            RelExpr::Unit => false,
        }
    }

    fn hoist(self, state: &State, id: usize, expr: Expr) -> Self {
        match expr {
            Expr::Subquery { expr } => {
                let att = expr.att();
                assert!(att.len() == 1);
                let input_col_id = att.iter().next().unwrap();
                let rhs = expr.map(state, vec![(id, Expr::ColRef { id: *input_col_id })]);
                self.apply(rhs)
            }
            Expr::Plus { left, right } => {
                // Hoist the left, hoist the right, then perform the plus.
                let lhs_id = state.next();
                let rhs_id = state.next();
                self.hoist(state, lhs_id, *left)
                    .hoist(state, rhs_id, *right)
                    .map(
                        state,
                        [(
                            id,
                            Expr::Plus {
                                left: Box::new(Expr::ColRef { id: lhs_id }),
                                right: Box::new(Expr::ColRef { id: rhs_id }),
                            },
                        )],
                    )
            }
            Expr::Int { .. } | Expr::ColRef { .. } => self.map(state, vec![(id, expr)]),
            x => unimplemented!("{:?}", x),
        }
    }

    fn map(self, state: &State, exprs: impl IntoIterator<Item = (usize, Expr)>) -> Self {
        let mut exprs: Vec<_> = exprs.into_iter().collect();

        if exprs.is_empty() {
            return self;
        }

        if let RelExpr::Map {
            input,
            exprs: mut existing,
        } = self
        {
            existing.append(&mut exprs);
            return RelExpr::Map {
                input,
                exprs: existing,
            };
        }

        for i in 0..exprs.len() {
            if exprs[i].1.has_subquery() {
                // We're going to hoist this one.
                let (id, expr) = exprs.swap_remove(i);
                return self.map(state, exprs).hoist(state, id, expr);
            }
        }

        RelExpr::Map {
            input: Box::new(self),
            exprs,
        }
    }

    fn apply(self, func: Self) -> Self {
        // Not correlated!
        if func.free().is_empty() {
            return self.join(func, vec![]);
        }

        RelExpr::Apply {
            input: Box::new(self),
            func: Box::new(func),
        }
    }

    fn project(self, cols: HashSet<usize>) -> Self {
        RelExpr::Project {
            src: Box::new(self),
            cols,
        }
    }

    fn att(&self) -> HashSet<usize> {
        match self {
            RelExpr::Scan { column_names, .. } => column_names.iter().cloned().collect(),
            RelExpr::Select { src, .. } => src.att(),
            RelExpr::Join { left, right, .. } => {
                let mut set = left.att();
                set.extend(right.att());
                set
            }
            RelExpr::Map { input, exprs } => {
                let mut set = input.att();
                set.extend(exprs.iter().map(|(id, _)| *id));
                set
            }
            RelExpr::Project { cols, .. } => cols.clone(),
            RelExpr::Unit => HashSet::new(),
            RelExpr::Apply { input, func } => {
                let mut set = input.att();
                set.extend(func.att());
                set
            }
        }
    }

    fn free(&self) -> HashSet<usize> {
        match self {
            RelExpr::Map { input, exprs } => {
                let mut set = input.free();
                for (_, expr) in exprs {
                    set.extend(expr.free());
                }
                set.difference(&input.att()).copied().collect()
            }
            _ => HashSet::new(),
        }
    }

    fn print(&self, indent: usize, out: &mut String) {
        match self {
            RelExpr::Scan {
                table_name,
                column_names,
            } => {
                out.push_str(&format!(
                    "{}-> scan({:?}, {:?})",
                    " ".repeat(indent),
                    table_name,
                    column_names
                ));
            }
            RelExpr::Select { src, predicates } => {
                out.push_str(&format!("{}-> select(", " ".repeat(indent)));
                let mut split = "";
                for predicate in predicates {
                    out.push_str(split);
                    predicate.print(indent, out);
                    split = " && "
                }
                out.push_str(")\n");
                src.print(indent + 2, out);
            }
            RelExpr::Join {
                left,
                right,
                predicates,
            } => {
                out.push_str(&format!("{}-> join(", " ".repeat(indent)));
                let mut split = "";
                for predicate in predicates {
                    out.push_str(split);
                    predicate.print(indent, out);
                    split = " && "
                }
                out.push_str(")\n");
                left.print(indent + 2, out);
                out.push('\n');
                right.print(indent + 2, out);
                out.push('\n');
            }
            RelExpr::Map { input, exprs } => {
                out.push_str(&format!("{}-> map(\n", " ".repeat(indent)));
                for (id, expr) in exprs {
                    out.push_str(&format!("{}    @{} <- ", " ".repeat(indent), id));
                    expr.print(indent, out);
                    out.push_str(",\n");
                }
                out.push_str(&format!("{})\n", " ".repeat(indent + 2)));
                input.print(indent + 2, out);
            }
            RelExpr::Project { src, cols } => {
                out.push_str(&format!("{}-> project\n", " ".repeat(indent)));
                src.print(indent + 2, out);
                out.push('\n');
                out.push_str(&format!("{}{:?},\n", " ".repeat(indent + 2), cols));
            }
            RelExpr::Unit => {
                out.push_str(&format!("{}-> unit\n", " ".repeat(indent)));
            }
            RelExpr::Apply { input, func } => {
                out.push_str(&format!("{}-> apply\n", " ".repeat(indent)));
                input.print(indent + 2, out);
                out.push_str(&format!("{}  λ.\n", " ".repeat(indent)));
                func.print(indent + 2, out);
            }
        }
    }
}

fn main() {
    let state = State::new();
    let left = RelExpr::scan("a".into(), vec![0, 1]);
    let right = RelExpr::scan("x".into(), vec![2, 3]);
    let sub = RelExpr::scan("x".into(), vec![4, 5]).project([4].into_iter().collect());

    let join = left
        .join(
            right,
            vec![
                Expr::col_ref(0).eq(Expr::int(100)),
                Expr::col_ref(1).eq(Expr::int(200)),
            ],
        )
        .map(
            &state,
            vec![
                (6, Expr::col_ref(0).plus(Expr::col_ref(2))),
                (
                    7,
                    Expr::Subquery {
                        expr: Box::new(sub.clone()),
                    },
                ),
                (
                    8,
                    Expr::int(3).plus(Expr::Subquery {
                        expr: Box::new(sub),
                    }),
                ),
            ],
        );

    let mut out = String::new();
    join.print(0, &mut out);

    println!("{}", out);
}
