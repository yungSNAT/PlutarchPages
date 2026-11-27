//! The fork's tests of the FormCalc parser.

use super::*;

fn bin(op: &'static str, a: Expr, b: Expr) -> Expr {
    Expr::Bin(op, Box::new(a), Box::new(b))
}

#[test]
fn loops_functions_and_blocks_parse_to_their_statements() {
    assert!(parse("for i = 1 to 5 do\nendfor").is_err_and(|e| e.contains("upto or downto")));
    assert_eq!(parse("foreach x in () do\nendfor"), Ok(vec![Stmt::Foreach { var: "x".into(), list: vec![], body: vec![] }]));
    assert!(parse("func empty() do\nendfunc").is_ok());
    assert!(parse("func pair(a, b) do\nreturn a\nendfunc").is_ok());
    // `do … end` is one block, kept as an always-true `if`.
    assert_eq!(parse("do\n1 + 2\nend"), Ok(vec![Stmt::If(vec![(Expr::Num(1.0), vec![Stmt::Expr(bin("+", Expr::Num(1.0), Expr::Num(2.0)))])], None)]));
    assert_eq!(parse("3 >= 2"), Ok(vec![Stmt::Expr(bin(">=", Expr::Num(3.0), Expr::Num(2.0)))]));
}

#[test]
fn only_a_signed_literal_index_is_relative() {
    // `[-1]` counts from the current instance; `[-idx]` is an absolute index worked out at run time.
    let ident = |s: &str| Box::new(Expr::Ident(s.into()));
    assert_eq!(parse("row[-1]"), Ok(vec![Stmt::Expr(Expr::Index(ident("row"), Idx::Rel(-1)))]));
    assert_eq!(parse("row[-idx]"), Ok(vec![Stmt::Expr(Expr::Index(ident("row"), Idx::Abs(Box::new(Expr::Neg(ident("idx"))))))]));
}

#[test]
fn errors_name_the_token_they_met() {
    assert!(parse("1 +").is_err_and(|e| e.contains("end of line")));
    assert_eq!(describe(&Tok::Num(4.0)), "number 4");
    assert_eq!(describe(&Tok::Str("hi".into())), "string \"hi\"");
    assert_eq!(describe(&Tok::Ident("foo".into())), "foo");
    assert_eq!(describe(&Tok::Op("+")), "\"+\"");
    assert_eq!(describe(&Tok::Newline), "end of line");
    assert_eq!(describe(&Tok::Eof), "end of script");
}
