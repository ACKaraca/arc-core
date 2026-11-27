use super::*;

fn schema(body: &str) -> Schema {
    parse(&format!("SCHEMA T;\n{body}\nEND_SCHEMA;")).unwrap()
}

fn expr(text: &str) -> Expr {
    let source = format!("SCHEMA T; TYPE A = INTEGER; WHERE {text}; END_TYPE; END_SCHEMA;");
    parse(&source)
        .unwrap()
        .types
        .remove(0)
        .where_rules
        .remove(0)
        .expr
}

fn name(n: &str) -> Box<Expr> {
    Box::new(Expr::Name(n.into()))
}

#[test]
fn aggregates_of_aggregates() {
    let s = schema(
        "ENTITY E; Points : LIST [2:?] OF LIST [2:?] OF UNIQUE P; \
         M : OPTIONAL ARRAY [1:3] OF OPTIONAL INTEGER; END_ENTITY;",
    );
    let e = &s.entities[0];
    let TypeSpec::Aggregate(outer) = &e.explicit[0].ty else {
        panic!()
    };
    assert_eq!(outer.kind, AggregateKind::List);
    let (lower, upper) = outer.bounds.clone().unwrap();
    assert_eq!(
        (lower.expr, upper.expr),
        (Expr::Integer(2), Expr::Indeterminate)
    );
    assert_eq!(upper.source, "?");
    let TypeSpec::Aggregate(inner) = &outer.element else {
        panic!()
    };
    assert!(inner.unique);
    assert_eq!(inner.element, TypeSpec::Named("P".into()));
    assert!(e.explicit[1].optional);
    let TypeSpec::Aggregate(array) = &e.explicit[1].ty else {
        panic!()
    };
    assert!(array.optional && array.kind == AggregateKind::Array);
}

#[test]
fn selects_of_selects_and_enumerations() {
    let s = schema(
        "TYPE A = SELECT (B, C); END_TYPE; TYPE B = SELECT (D); END_TYPE;\n\
         TYPE C = ENUMERATION OF (X, Y); END_TYPE;\n\
         TYPE D = EXTENSIBLE ENUMERATION; END_TYPE;\n\
         TYPE F = ENUMERATION BASED_ON D WITH (Z); END_TYPE;\n\
         TYPE G = EXTENSIBLE GENERIC_ENTITY SELECT BASED_ON A WITH (H); END_TYPE;",
    );
    assert_eq!(
        s.types[0].body,
        TypeBody::Select {
            extensible: false,
            based_on: None,
            members: vec!["B".into(), "C".into()]
        }
    );
    assert!(matches!(&s.types[2].body, TypeBody::Enumeration { items, .. } if items.len() == 2));
    assert!(matches!(
        &s.types[3].body,
        TypeBody::Enumeration { extensible: true, items, .. } if items.is_empty()
    ));
    assert!(matches!(
        &s.types[4].body,
        TypeBody::Enumeration { based_on: Some(b), items, .. } if b == "D" && items[0] == "Z"
    ));
    assert!(matches!(
        &s.types[5].body,
        TypeBody::Select {
            extensible: true,
            based_on: Some(_),
            ..
        }
    ));
}

#[test]
fn entity_sections_and_redeclarations() {
    let s = schema(
        "ENTITY Sub ABSTRACT SUPERTYPE OF (ONEOF (A, B) ANDOR C) SUBTYPE OF (Base);\n\
           X, Y : REAL;\n\
         DERIVE\n\
           SELF\\Base.Dim : INTEGER := 3;\n\
           Len : REAL := SQRT(X ** 2 + Y ** 2);\n\
         INVERSE\n\
           Users : SET [0:?] OF User FOR Used;\n\
           Owner : Other FOR Base.Item;\n\
         UNIQUE\n\
           UR1 : X, SELF\\Base.Id;\n\
         WHERE\n\
           WR1 : X > 0.;\n\
           SIZEOF(Users) <= 1;\n\
         END_ENTITY;",
    );
    let e = &s.entities[0];
    assert!(e.is_abstract);
    assert!(matches!(e.supertype_of, Some(Supertype::AndOr(_, _))));
    assert_eq!(e.subtype_of, vec!["Base".to_owned()]);
    assert_eq!(e.explicit.len(), 2);
    assert_eq!(
        e.derived[0].name,
        AttrName::Redeclared {
            entity: "Base".into(),
            attribute: "Dim".into(),
            renamed: None
        }
    );
    assert_eq!(e.derived[1].source, "SQRT(X ** 2 + Y ** 2)");
    assert_eq!(
        e.inverse[0].aggregate.as_ref().map(|a| a.0),
        Some(AggregateKind::Set)
    );
    assert_eq!(e.inverse[1].for_entity.as_deref(), Some("Base"));
    assert_eq!(e.unique[0].attributes, vec!["X", "SELF\\Base.Id"]);
    assert_eq!(e.where_rules[0].label.as_deref(), Some("WR1"));
    assert_eq!(e.where_rules[1].label, None);
    assert_eq!(e.where_rules[1].source, "SIZEOF(Users) <= 1");
}

#[test]
fn query_group_qualifiers_and_precedence() {
    assert_eq!(
        expr("SIZEOF(QUERY(T <* S | T.K = E.A)) = 1"),
        Expr::Binary(
            BinaryOp::Eq,
            Box::new(Expr::Call(
                "SIZEOF".into(),
                vec![Expr::Query {
                    variable: "T".into(),
                    aggregate: name("S"),
                    condition: Box::new(Expr::Binary(
                        BinaryOp::Eq,
                        Box::new(Expr::Attribute(name("T"), "K".into())),
                        Box::new(Expr::Attribute(name("E"), "A".into())),
                    )),
                }]
            )),
            Box::new(Expr::Integer(1)),
        )
    );
    assert_eq!(
        expr("EXISTS(SELF\\IfcX.Attr[1])"),
        Expr::Call(
            "EXISTS".into(),
            vec![Expr::Index(
                Box::new(Expr::Attribute(
                    Box::new(Expr::Group(name("SELF"), "IfcX".into())),
                    "Attr".into()
                )),
                Box::new(Expr::Integer(1)),
                None
            )]
        )
    );
    // NOT binds tighter than AND, AND tighter than OR.
    assert_eq!(
        expr("NOT a AND b OR c"),
        Expr::Binary(
            BinaryOp::Or,
            Box::new(Expr::Binary(
                BinaryOp::And,
                Box::new(Expr::Unary(UnaryOp::Not, name("a"))),
                name("b")
            )),
            name("c")
        )
    );
    assert!(matches!(
        expr("{0 <= x < 1}"),
        Expr::Interval {
            low_inclusive: true,
            high_inclusive: false,
            ..
        }
    ));
    assert!(matches!(
        expr("'IFC4.IFCX' IN TYPEOF(SELF)"),
        Expr::Binary(BinaryOp::In, _, _)
    ));
    assert_eq!(
        expr("'it''s' = x"),
        Expr::Binary(BinaryOp::Eq, Box::new(Expr::Str("it's".into())), name("x"))
    );
    assert!(matches!(
        expr("[a, b : 2] :<>: c"),
        Expr::Binary(BinaryOp::InstanceNe, _, _)
    ));
}

#[test]
fn functions_and_rules() {
    let s = schema(
        "FUNCTION F (Lis : LIST [1:?] OF LIST [1:?] OF GENERIC : T; Low, U : INTEGER)\n\
           : ARRAY [Low:U] OF GENERIC : T;\n\
         LOCAL\n  Res : ARRAY [Low:U] OF GENERIC : T; (* note *)\n  i : INTEGER := 0;\nEND_LOCAL;\n\
           REPEAT i := 1 TO U BY 1;\n\
             IF i > 2 THEN ESCAPE; ELSE Res[i] := Lis[i][1]; END_IF;\n\
           END_REPEAT;\n\
           CASE i OF 1, 2 : RETURN (Res); OTHERWISE : ; END_CASE;\n\
           INSERT(Res, ?, 0);\n\
           RETURN (?);\n\
         END_FUNCTION;\n\
         RULE R FOR (E);\n  WHERE\n  WR1 : SIZEOF(E) <= 1;\nEND_RULE;",
    );
    let f = &s.functions[0];
    assert_eq!(f.parameters.len(), 3);
    assert_eq!(f.locals.len(), 2);
    assert_eq!(f.body.len(), 4);
    assert!(f.source.starts_with("FUNCTION F") && f.source.ends_with("END_FUNCTION;"));
    assert_eq!(s.rules[0].where_rules.len(), 1);
    assert!(s.rules[0].source.ends_with("END_RULE;"));
}

#[test]
fn errors_report_positions_and_never_panic() {
    let error = parse("SCHEMA T;\nENTITY E;\n  A : ;\nEND_ENTITY;\nEND_SCHEMA;").unwrap_err();
    assert_eq!(error.pos, Pos { line: 3, column: 7 });
    assert!(error.message.contains("expected a name"), "{error}");
    let error = parse("SCHEMA T; USE FROM X; END_SCHEMA;").unwrap_err();
    assert!(error.message.contains("not supported"));
    // Deep nesting is refused, not overflowed.
    let deep = format!(
        "SCHEMA T; TYPE A = INTEGER; WHERE {}1{}; END_TYPE; END_SCHEMA;",
        "(".repeat(5000),
        ")".repeat(5000)
    );
    assert!(parse(&deep).unwrap_err().message.contains("nesting"));
    // Truncations of a valid schema all fail cleanly.
    let valid = "SCHEMA T; ENTITY E; A : LIST [1:?] OF INTEGER; WHERE W : SIZEOF(A) > {1 <= 2 < 3}; END_ENTITY; END_SCHEMA;";
    assert!(parse(valid).is_ok());
    for cut in 0..valid.len() - 1 {
        assert!(parse(&valid[..cut]).is_err());
    }
}
