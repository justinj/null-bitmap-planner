use std::{backtrace, cell::RefCell, collections::HashSet, rc::Rc};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Rule {
    Hoist,
    Decorrelate,
}

#[derive(Debug, Clone)]
struct State {
    next_id: Rc<RefCell<usize>>,
    enabled_rules: Rc<RefCell<HashSet<Rule>>>,
}

impl State {
    fn new() -> Self {
        State {
            next_id: Rc::new(RefCell::new(0)),
            enabled_rules: Rc::new(RefCell::new(HashSet::new())),
        }
    }

    fn next(&self) -> usize {
        let id = *self.next_id.borrow();
        *self.next_id.borrow_mut() += 1;
        id
    }

    fn enable(&self, rule: Rule) {
        let mut enabled_rules = self.enabled_rules.borrow_mut();
        enabled_rules.insert(rule);
    }

    fn enabled(&self, rule: Rule) -> bool {
        self.enabled_rules.borrow().contains(&rule)
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
    Project {
        src: Box<RelExpr>,
        cols: HashSet<usize>,
    },
    Map {
        input: Box<RelExpr>,
        exprs: Vec<(usize, Expr)>,
    },
    FlatMap {
        input: Box<RelExpr>,
        func: Box<RelExpr>,
    },
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
            RelExpr::FlatMap { input, func } => input.has_subquery() || func.has_subquery(),
        }
    }

    fn hoist(self, state: &State, id: usize, expr: Expr) -> Self {
        match expr {
            Expr::Subquery { expr } => {
                let att = expr.att();
                assert!(att.len() == 1);
                let input_col_id = att.iter().next().unwrap();
                let rhs = expr.map(state, vec![(id, Expr::ColRef { id: *input_col_id })]);
                self.flatmap(state, rhs)
            }
            Expr::Plus { left, right } => {
                // Hoist the left, hoist the right, then perform the plus.
                let lhs_id = state.next();
                let rhs_id = state.next();
                let att = self.att();
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
                    .project(state, att.into_iter().chain([id].into_iter()).collect())
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

        // if let RelExpr::Map {
        //     input,
        //     exprs: mut existing,
        // } = self
        // {
        //     existing.append(&mut exprs);
        //     return RelExpr::Map {
        //         input,
        //         exprs: existing,
        //     };
        // }

        if state.enabled(Rule::Hoist) {
            for i in 0..exprs.len() {
                // Only hoist expressions with subqueries.
                if exprs[i].1.has_subquery() {
                    let (id, expr) = exprs.swap_remove(i);
                    return self.map(state, exprs).hoist(state, id, expr);
                }
            }
        }

        RelExpr::Map {
            input: Box::new(self),
            exprs,
        }
    }

    fn flatmap(self, state: &State, func: Self) -> Self {
        if state.enabled(Rule::Decorrelate) {
            // Not correlated!
            if func.free().is_empty() {
                return self.join(func, vec![]);
            }

            if let RelExpr::Project { src, mut cols } = func {
                cols.extend(self.att());
                return self.flatmap(state, *src).project(state, cols);
            }

            // Pull up Maps.
            if let RelExpr::Map { input, exprs } = func {
                return self.flatmap(state, *input).map(state, exprs);
            }
        }

        RelExpr::FlatMap {
            input: Box::new(self),
            func: Box::new(func),
        }
    }

    fn project(self, state: &State, cols: HashSet<usize>) -> Self {
        // Push project into Map if we can.
        if let RelExpr::Map { exprs, .. } = &self {
            let map_required_cols: HashSet<_> =
                exprs.iter().flat_map(|(_, expr)| expr.free()).collect();
            if cols.is_subset(&map_required_cols) {
                // Guaranteed to work.
                if let RelExpr::Map { input, exprs } = self {
                    return input.project(state, cols).map(state, exprs);
                }
            }
        }

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
            RelExpr::FlatMap { input, func } => {
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
            RelExpr::FlatMap { input, func } => {
                let mut set = input.free();
                set.extend(func.free());
                set.difference(&input.att()).copied().collect()
            }
            RelExpr::Scan { .. } => HashSet::new(),
            RelExpr::Select { src, predicates } => {
                let mut set = src.free();
                for expr in predicates {
                    set.extend(expr.free());
                }
                set.difference(&src.att()).copied().collect()
            }
            RelExpr::Join {
                left,
                right,
                predicates,
            } => {
                let mut set = left.free();
                set.extend(right.free());
                for expr in predicates {
                    set.extend(expr.free());
                }
                set.difference(&left.att().union(&right.att()).copied().collect())
                    .copied()
                    .collect()
            }
            RelExpr::Project { src, .. } => src.free(),
        }
    }

    fn print(&self, indent: usize, out: &mut String) {
        match self {
            RelExpr::Scan {
                table_name,
                column_names,
            } => {
                out.push_str(&format!(
                    "{}-> scan({:?}, {:?})\n",
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
                right.print(indent + 2, out);
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
                out.push_str(&format!("{}-> project({:?})\n", " ".repeat(indent), cols));
                src.print(indent + 2, out);
            }
            RelExpr::FlatMap { input, func } => {
                out.push_str(&format!("{}-> flatmap\n", " ".repeat(indent)));
                input.print(indent + 2, out);
                out.push_str(&format!("{}  λ.{:?}\n", " ".repeat(indent), func.free()));
                func.print(indent + 2, out);
            }
        }
    }
}

fn main() {
    let state = State::new();
    state.enable(Rule::Hoist);
    state.enable(Rule::Decorrelate);

    let a = state.next();
    let b = state.next();
    let x = state.next();
    let y = state.next();

    let sum_col = state.next();

    let v = RelExpr::scan("a".into(), vec![a, b]).map(
        &state,
        vec![
            // (
            //     state.next(),
            //     Expr::int(3).plus(Expr::Subquery {
            //         expr: Box::new(
            //             RelExpr::scan("x".into(), vec![x, y]).project([x].into_iter().collect()),
            //         ),
            //     }),
            // ),
            (
                state.next(),
                Expr::int(4).plus(Expr::Subquery {
                    expr: Box::new(
                        RelExpr::scan("x".into(), vec![x, y])
                            .project(&state, [x].into_iter().collect())
                            .map(&state, [(sum_col, Expr::col_ref(x).plus(Expr::col_ref(a)))])
                            .project(&state, [sum_col].into_iter().collect()),
                    ),
                }),
            ),
        ],
    );

    // let v = RelExpr::scan("a".into(), vec![a, b]).map(
    //     &state,
    //     vec![(
    //         state.next(),
    //         Expr::Subquery {
    //             expr: Box::new(
    //                 RelExpr::scan("x".into(), vec![x, y])
    //                     .project(&state, [x].into_iter().collect()),
    //             ),
    //         },
    //     )],
    // );

    let mut out = String::new();
    v.print(0, &mut out);

    println!("{}", out);
}
