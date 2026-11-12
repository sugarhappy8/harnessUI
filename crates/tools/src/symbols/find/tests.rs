use super::*;
use SymbolKind::*;

/// The one definition called `name`.
fn one<'a>(all: &'a [Symbol], name: &str) -> &'a Symbol {
    let found: Vec<&Symbol> = all.iter().filter(|s| &*s.name == name).collect();
    assert_eq!(
        found.len(),
        1,
        "{name}: found {} times in {:?}",
        found.len(),
        names(all)
    );
    found[0]
}

fn names(all: &[Symbol]) -> Vec<&str> {
    all.iter().map(|s| &*s.name).collect()
}

/// Asserts kind, container and line range of the one definition called `name`.
#[track_caller]
fn check(
    all: &[Symbol],
    name: &str,
    kind: SymbolKind,
    container: Option<&str>,
    line: u32,
    end: u32,
) {
    let s = one_kind(all, name, kind);
    assert_eq!(
        (s.kind, s.container.as_deref(), s.line, s.end_line),
        (kind, container, line, end),
        "{name}"
    );
}

#[track_caller]
fn absent(all: &[Symbol], unwanted: &[&str]) {
    for name in unwanted {
        assert!(
            !all.iter().any(|s| &*s.name == *name),
            "{name} must not be found; found {:?}",
            names(all)
        );
    }
}

#[test]
fn languages_are_told_by_the_file_extension() {
    for (path, lang) in [
        ("src/lib.rs", Lang::Rust),
        ("a/b.TS", Lang::TypeScript),
        ("App.tsx", Lang::TypeScript),
        ("x.mts", Lang::TypeScript),
        ("x.js", Lang::JavaScript),
        ("x.jsx", Lang::JavaScript),
        ("x.mjs", Lang::JavaScript),
        ("calc.py", Lang::Python),
        ("main.go", Lang::Go),
        ("A.java", Lang::Java),
        ("A.cs", Lang::CSharp),
        ("A.kt", Lang::Kotlin),
        ("A.swift", Lang::Swift),
        ("a.c", Lang::C),
        // Read as C++, which also covers C: many C++ headers are named `.h`.
        ("a.h", Lang::Cpp),
        ("a.cpp", Lang::Cpp),
        ("a.hpp", Lang::Cpp),
        ("a.cc", Lang::Cpp),
    ] {
        assert_eq!(Lang::from_path(path), Some(lang), "{path}");
    }
    for path in [
        "notes.txt",
        "Makefile",
        "a.rs.bak",
        ".rs",
        "dir.rs/readme",
        "",
    ] {
        assert_eq!(Lang::from_path(path), None, "{path}");
    }
}

const RUST: &str = r##"//! Module docs: fn not_here() {}
use std::fmt;

/// Adds two numbers.
#[inline]
pub fn add(a: i32, b: i32) -> i32 {
    let s = "fn fake_in_string() {";
    // fn fake_in_comment() {}
    helper(a) + b
}

pub(crate) async fn fetch<T>(url: &str) -> Result<T, Error>
where
    T: Default,
{
    todo!()
}

pub const unsafe extern "C" fn raw(p: *const u8) -> u8 { *p }

#[derive(Debug, Clone)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

struct Unit;
struct Pair(i32, i32);

pub enum Shape {
    Circle { r: f32 },
    Square(f32),
}

pub trait Area: fmt::Debug {
    fn area(&self) -> f32;
    fn doubled(&self) -> f32 {
        self.area() * 2.0
    }
}

impl Point {
    pub fn new(x: i32, y: i32) -> Self {
        fn clamp(v: i32) -> i32 {
            v.max(0)
        }
        Point { x: clamp(x), y }
    }
}

impl<T: fmt::Debug> Area for Wrapper<T> {
    fn area(&self) -> f32 { 0.0 }
}

unsafe impl Send for Point {}

mod inner {
    pub fn deep() {}
}
mod elsewhere;

pub const LIMIT: usize = 10;
static mut COUNTER: u32 = 0;
const TABLE: &[&str] = &[
    "a",
    "b",
];
pub type Pairs = Vec<(i32, i32)>;

fn takes(
    f: impl Fn(i32) -> i32,
) -> Result<
    impl Iterator<Item = i32>,
    Error,
> {
    todo!()
}

macro_rules! make {
    ($name:ident) => {
        fn $name() {}
    };
}
"##;

#[test]
fn rust_definitions_with_kind_and_line_range() {
    let all = find_symbols(RUST, Lang::Rust);
    check(&all, "add", Function, None, 6, 10);
    assert_eq!(one(&all, "add").lead, 4, "doc comment and attribute lead");
    check(&all, "fetch", Function, None, 12, 17);
    check(&all, "raw", Function, None, 19, 19);
    check(&all, "Point", Struct, None, 22, 25);
    assert_eq!(one_kind(&all, "Point", Struct).lead, 21);
    check(&all, "Unit", Struct, None, 27, 27);
    check(&all, "Pair", Struct, None, 28, 28);
    check(&all, "Shape", Enum, None, 30, 33);
    check(&all, "Area", Trait, None, 35, 40);
    check(&all, "doubled", Method, Some("Area"), 37, 39);
    check(&all, "new", Method, Some("Point"), 43, 48);
    // A function inside a function is a plain function.
    check(&all, "clamp", Function, None, 44, 46);
    check(&all, "inner", Module, None, 57, 59);
    check(&all, "deep", Function, None, 58, 58);
    check(&all, "LIMIT", Const, None, 62, 62);
    check(&all, "COUNTER", Const, None, 63, 63);
    check(&all, "TABLE", Const, None, 64, 67);
    check(&all, "Pairs", Type, None, 68, 68);
    check(&all, "takes", Function, None, 70, 77);

    // `area` is declared in the trait and defined in the impl.
    let areas: Vec<_> = all.iter().filter(|s| &*s.name == "area").collect();
    assert_eq!(areas.len(), 2);
    assert_eq!(
        (areas[0].kind, areas[0].container.as_deref(), areas[0].line),
        (Method, Some("Area"), 36)
    );
    assert_eq!(
        (areas[1].kind, areas[1].container.as_deref(), areas[1].line),
        (Method, Some("Wrapper"), 52)
    );

    // Impl blocks: the type is the name; a trait impl carries the trait.
    let impls: Vec<_> = all.iter().filter(|s| s.kind == Impl).collect();
    assert_eq!(
        impls
            .iter()
            .map(|s| (&*s.name, s.container.as_deref(), s.line, s.end_line))
            .collect::<Vec<_>>(),
        vec![
            ("Point", None, 42, 49),
            ("Wrapper", Some("Area"), 51, 53),
            ("Point", Some("Send"), 55, 55),
        ]
    );

    absent(
        &all,
        &[
            "not_here",
            "fake_in_string",
            "fake_in_comment",
            "helper",
            "elsewhere",
            "Fn",
            "Iterator",
            "Error",
            "$name",
            "name",
            "make",
        ],
    );
}

#[test]
fn rust_calls_and_expressions_are_not_definitions() {
    let text = "fn main() {\n    let fn_ptr: fn(u8) -> u8 = id;\n    struct_like(1);\n    let x = Foo::new();\n    callback(|| mod_name());\n    type_of(x);\n}\n";
    let all = find_symbols(text, Lang::Rust);
    assert_eq!(names(&all), vec!["main"]);
    check(&all, "main", Function, None, 1, 7);
}

const TYPESCRIPT: &str = r##"import { memo } from "react";

/** Adds. */
export function add(a: number, b: number): number {
  const s = `function fakeInTemplate() { ${a} }`;
  // function fakeInComment() {}
  return helper(a) + b;
}

export async function load<T>(url: string): Promise<{ ok: boolean }> {
  return fetchIt(url);
}

function* gen() {
  yield 1;
}

export default class Store extends Base implements Thing {
  private items: string[] = [];
  static count = 0;

  constructor(private readonly name: string) {
    super();
  }

  add(item: string): void;
  add(item: string, twice?: boolean): void {
    if (twice) {
      this.items.push(item);
    }
    this.items.push(item);
  }

  static async create(): Promise<Store> {
    return new Store("x");
  }

  get size(): number {
    return this.items.length;
  }

  onClick = (e: Event) => {
    console.log(e);
  };
}

export interface Shape {
  area(): number;
  name: string;
}

export type Id = string | number;
export type Mode =
  | "a"
  | "b"

export enum Color {
  Red,
  Green,
}
export const enum Flag { On, Off }

export const double = (n: number) => n * 2;
export const triple = async (
  n: number,
): Promise<number> => {
  return n * 3;
};
export const Item = memo(function Item(props: { id: Id }) {
  return <li>{props.id} isn't {"function fakeInJsx() {"}</li>;
});
const local = (a: string) => {
  return a;
};
export const LIMIT = 10;
export const ids = items.map((i) => i.id);
export const sum = (1 + 2) * 3;
export const pattern = /function fakeInRegex\(\) {/;
const config = {
  method() {
    return 1;
  },
};
declare namespace Api {
  function call(): void;
}
"##;

#[test]
fn typescript_definitions_with_kind_and_line_range() {
    let all = find_symbols(TYPESCRIPT, Lang::TypeScript);
    check(&all, "add", Function, None, 4, 8);
    assert_eq!(one_fn(&all, "add").lead, 3);
    check(&all, "load", Function, None, 10, 12);
    check(&all, "gen", Function, None, 14, 16);
    check(&all, "Store", Class, None, 18, 45);
    check(&all, "constructor", Method, Some("Store"), 22, 24);
    check(&all, "create", Method, Some("Store"), 34, 36);
    check(&all, "size", Method, Some("Store"), 38, 40);
    check(&all, "onClick", Method, Some("Store"), 42, 44);
    check(&all, "Shape", Interface, None, 47, 50);
    check(&all, "Id", Type, None, 52, 52);
    check(&all, "Mode", Type, None, 53, 55);
    check(&all, "Color", Enum, None, 57, 60);
    check(&all, "Flag", Enum, None, 61, 61);
    check(&all, "double", Function, None, 63, 63);
    check(&all, "triple", Function, None, 64, 68);
    check(&all, "Item", Function, None, 69, 71);
    check(&all, "local", Function, None, 72, 74);
    check(&all, "Api", Module, None, 84, 86);

    // The overload signature (line 26) is skipped; the method with a body is found.
    let adds: Vec<_> = all
        .iter()
        .filter(|s| &*s.name == "add" && s.kind == Method)
        .collect();
    assert_eq!(adds.len(), 1);
    assert_eq!(
        (adds[0].container.as_deref(), adds[0].line, adds[0].end_line),
        (Some("Store"), 27, 32)
    );

    absent(
        &all,
        &[
            "fakeInTemplate",
            "fakeInComment",
            "fakeInJsx",
            "fakeInRegex",
            "helper",
            "fetchIt",
            "if",
            "LIMIT",
            "ids",
            "sum",
            "pattern",
            "config",
            "method",
            "items",
            "count",
            "area",
            "name",
            "super",
        ],
    );
}

/// The function called `name` (a method may share the name).
fn one_fn<'a>(all: &'a [Symbol], name: &str) -> &'a Symbol {
    let found: Vec<&Symbol> = all
        .iter()
        .filter(|s| &*s.name == name && s.kind == Function)
        .collect();
    assert_eq!(found.len(), 1, "{name}");
    found[0]
}

#[test]
fn javascript_functions_classes_and_methods() {
    let text = r##"'use strict';
const path = require('path');

function plain(a, b) {
  return a + b;
}

class Queue extends Base {
  #items = [];
  static of(...xs) {
    return new Queue(xs);
  }
  async *drain() {
    yield* this.#items;
  }
  #secret() { return 1; }
}

module.exports = { plain, Queue };
export const handler = async (req, res) => {
  res.end(plain(1, 2));
};
describe("queue", () => {
  function setup() {
    return new Queue();
  }
  it("works", () => { setup(); });
});
"##;
    let all = find_symbols(text, Lang::JavaScript);
    check(&all, "plain", Function, None, 4, 6);
    check(&all, "Queue", Class, None, 8, 17);
    check(&all, "of", Method, Some("Queue"), 10, 12);
    check(&all, "drain", Method, Some("Queue"), 13, 15);
    check(&all, "secret", Method, Some("Queue"), 16, 16);
    check(&all, "handler", Function, None, 20, 22);
    // Inside a callback, so inside brackets: still a definition.
    check(&all, "setup", Function, None, 24, 26);
    absent(&all, &["require", "describe", "it", "exports", "path"]);
}

const PYTHON: &str = r##"#!/usr/bin/env python3
"""Module docstring.

def fake_in_docstring():
    pass
"""
import os


def add(a, b):
    """Adds.

    class FakeInDocstring:
    """
    # def fake_in_comment(): pass
    total = a + b

    return total


@decorator
@other(arg=1)
async def fetch(
    url,
    timeout=10,
):
    x = call_something(url)
    return x


class Shape(Base):
    sides = 0

    def area(self):
        def helper(x):
            return x * 2

        return helper(self.sides)

    @property
    def name(self): return "shape"

    class Meta:
        ordering = ()


def last():
    s = '''
def fake_in_string():
    '''
    return s
"##;

#[test]
fn python_definitions_end_by_indentation() {
    let all = find_symbols(PYTHON, Lang::Python);
    check(&all, "add", Function, None, 10, 18);
    check(&all, "fetch", Function, None, 23, 28);
    assert_eq!(one(&all, "fetch").lead, 21, "decorators lead");
    check(&all, "Shape", Class, None, 31, 44);
    check(&all, "area", Method, Some("Shape"), 34, 38);
    // A function inside a method is a plain function.
    check(&all, "helper", Function, None, 35, 36);
    check(&all, "name", Method, Some("Shape"), 41, 41);
    assert_eq!(one(&all, "name").lead, 40);
    check(&all, "Meta", Class, None, 43, 44);
    check(&all, "last", Function, None, 47, 51);
    absent(
        &all,
        &[
            "fake_in_docstring",
            "FakeInDocstring",
            "fake_in_comment",
            "fake_in_string",
            "call_something",
            "decorator",
            "other",
        ],
    );
}

#[test]
fn python_without_a_final_newline_and_with_tabs() {
    let text = "class A:\n\tdef f(self):\n\t\treturn 1\n\n\tdef g(self):\n\t\treturn 2";
    let all = find_symbols(text, Lang::Python);
    check(&all, "A", Class, None, 1, 6);
    check(&all, "f", Method, Some("A"), 2, 3);
    check(&all, "g", Method, Some("A"), 5, 6);
}

const GO: &str = r##"package main

import "fmt"

// Add adds.
func Add(a, b int) int {
	s := `func FakeInRaw() {
}`
	// func FakeInComment() {}
	return helper(a) + b + len(s)
}

func (p *Point) Scale(f float64) {
	p.X *= f
}

func (s Stack[T]) Peek() (T, bool) {
	var zero T
	return zero, false
}

func Map[T, U any](xs []T, f func(T) U) []U {
	return nil
}

func Loose() map[string]interface{} {
	return nil
}

type Point struct {
	X, Y float64
}

type Shape interface {
	Area() float64
}

type ID int
type Handler func(w int) error

type (
	Pair struct {
		A, B int
	}
	Name string
)

func main() {
	f := func() {
		fmt.Println("func FakeInString() {")
	}
	f()
	go func() {}()
}
"##;

#[test]
fn go_functions_methods_and_types() {
    let all = find_symbols(GO, Lang::Go);
    check(&all, "Add", Function, None, 6, 11);
    assert_eq!(one(&all, "Add").lead, 5);
    check(&all, "Scale", Method, Some("Point"), 13, 15);
    check(&all, "Peek", Method, Some("Stack"), 17, 20);
    check(&all, "Map", Function, None, 22, 24);
    check(&all, "Loose", Function, None, 26, 28);
    check(&all, "Point", Struct, None, 30, 32);
    check(&all, "Shape", Interface, None, 34, 36);
    check(&all, "ID", Type, None, 38, 38);
    check(&all, "Handler", Type, None, 39, 39);
    check(&all, "Pair", Struct, None, 42, 44);
    check(&all, "Name", Type, None, 45, 45);
    check(&all, "main", Function, None, 48, 54);
    absent(
        &all,
        &[
            "FakeInRaw",
            "FakeInComment",
            "FakeInString",
            "helper",
            "Println",
            "f",
            "Area",
        ],
    );
}

const JAVA: &str = r##"package com.example;

import java.util.List;

/** A shape. */
@Deprecated
public abstract class Shape<T extends Number> implements Comparable<Shape<T>> {
    private final List<String> names = List.of("a", "b");
    private int sides = compute(3);
    static final String TEXT = """
        void fakeInTextBlock() {
        """;

    public Shape(int sides) {
        this.sides = sides;
    }

    @Override
    public int compareTo(Shape<T> o) {
        if (o == null) {
            return helper(1);
        }
        return Integer.compare(sides, o.sides);
    }

    public abstract double area();

    protected static <U> List<U> wrap(
            U value,
            int times) throws IllegalStateException {
        // void fakeInComment() {}
        return List.of(value);
    }

    @SuppressWarnings("unchecked") private T cast(Object o) { return (T) o; }

    enum Kind {
        ROUND(0),
        SQUARE(4) {
            @Override int corners() { return 4; }
        };

        private final int n;
        Kind(int n) { this.n = n; }
        int corners() { return n; }
    }

    interface Visitor {
        void visit(Shape<?> s);
        default void done() {}
    }

    record Size(int w, int h) {
        int area() { return w * h; }
    }
}
"##;

#[test]
fn java_types_and_methods() {
    let all = find_symbols(JAVA, Lang::Java);
    check(&all, "Shape", Class, None, 7, 56);
    assert_eq!(one_kind(&all, "Shape", Class).lead, 5);
    check(&all, "compareTo", Method, Some("Shape"), 19, 24);
    assert_eq!(one(&all, "compareTo").lead, 18, "annotation leads");
    check(&all, "wrap", Method, Some("Shape"), 28, 33);
    check(&all, "cast", Method, Some("Shape"), 35, 35);
    check(&all, "Kind", Enum, None, 37, 46);
    check(&all, "Visitor", Interface, None, 48, 51);
    check(&all, "done", Method, Some("Visitor"), 50, 50);
    check(&all, "Size", Class, None, 53, 55);

    let ctors: Vec<_> = all
        .iter()
        .filter(|s| s.kind == Method && s.container.as_deref() == Some(&*s.name))
        .map(|s| (&*s.name, s.line, s.end_line))
        .collect();
    assert_eq!(ctors, vec![("Shape", 14, 16), ("Kind", 44, 44)]);

    // `corners` of the enum (line 45); the one in the constant's body is deeper.
    check(&all, "corners", Method, Some("Kind"), 45, 45);
    let areas: Vec<_> = all
        .iter()
        .filter(|s| &*s.name == "area")
        .map(|s| (s.container.as_deref(), s.line))
        .collect();
    // The abstract method (line 26) has no body and is skipped.
    assert_eq!(areas, vec![(Some("Size"), 54)]);

    absent(
        &all,
        &[
            "fakeInTextBlock",
            "fakeInComment",
            "compute",
            "of",
            "helper",
            "compare",
            "visit",
            "ROUND",
            "SQUARE",
            "if",
            "names",
            "sides",
        ],
    );
}

/// The one definition called `name` of `kind`.
fn one_kind<'a>(all: &'a [Symbol], name: &str, kind: SymbolKind) -> &'a Symbol {
    let found: Vec<&Symbol> = all
        .iter()
        .filter(|s| &*s.name == name && s.kind == kind)
        .collect();
    assert_eq!(found.len(), 1, "{name}");
    found[0]
}

const CSHARP: &str = r##"using System;

namespace Shop.Models
{
    #region Types
    [Serializable]
    public sealed class Cart : IDisposable
    {
        private readonly List<Item> _items = new List<Item>();
        public int Count => _items.Count;
        public string Name { get; set; } = Describe(1);
        const string Text = @"void FakeInVerbatim() {
        }";

        public Cart(int capacity)
        {
            _items = new List<Item>(capacity);
        }

        [Obsolete]
        public void Add(Item item)
        {
            if (item == null) throw new ArgumentNullException(nameof(item));
            _items.Add(item);
        }

        public int Total(int tax) => Sum(_items) + tax;

        public static async Task<Cart?> LoadAsync(
            string path,
            CancellationToken ct)
        {
            // void FakeInComment() {}
            return await Read(path, ct);
        }

        public void Dispose() { }
        public abstract void Later();
    }
    #endregion

    public struct Money { public decimal Amount; }
    public interface IPriced { decimal Price(); }
    public enum Status { Open, Closed }
    public record Item(string Name, decimal Price);
    public record class Order(int Id)
    {
        public bool Valid() { return Id > 0; }
    }
}
"##;

#[test]
fn csharp_types_and_methods() {
    let all = find_symbols(CSHARP, Lang::CSharp);
    check(&all, "Shop.Models", Module, None, 3, 50);
    check(&all, "Cart", Class, None, 7, 39);
    assert_eq!(one_kind(&all, "Cart", Class).lead, 5);
    check(&all, "Add", Method, Some("Cart"), 21, 25);
    assert_eq!(one(&all, "Add").lead, 20, "attribute leads");
    check(&all, "Total", Method, Some("Cart"), 27, 27);
    check(&all, "LoadAsync", Method, Some("Cart"), 29, 35);
    check(&all, "Dispose", Method, Some("Cart"), 37, 37);
    check(&all, "Money", Struct, None, 42, 42);
    check(&all, "IPriced", Interface, None, 43, 43);
    check(&all, "Status", Enum, None, 44, 44);
    check(&all, "Item", Class, None, 45, 45);
    check(&all, "Order", Class, None, 46, 49);
    check(&all, "Valid", Method, Some("Order"), 48, 48);
    let ctor = one_kind(&all, "Cart", Method);
    assert_eq!((ctor.line, ctor.end_line), (15, 18));
    absent(
        &all,
        &[
            "FakeInVerbatim",
            "FakeInComment",
            "Count",
            "Name",
            "Describe",
            "Sum",
            "Read",
            "Later",
            "nameof",
            "ArgumentNullException",
            "Price",
            "if",
        ],
    );
}

const KOTLIN: &str = r##"package shop

import kotlin.math.max

/** Adds. */
fun add(a: Int, b: Int): Int {
    val s = """
        fun fakeInRawString() {
    """
    // fun fakeInComment() {}
    return helper(a) + b
}

fun String.shout(): String = uppercase() + "!"

fun <T> List<T>.second(): T =
    this[1]

inline fun <reified T> typed(value: Any): T? = value as? T

data class Point(val x: Int, val y: Int)

sealed class Shape(
    val name: String,
) : Comparable<Shape> {
    abstract fun area(): Double

    override fun compareTo(other: Shape): Int {
        return max(1, 2)
    }

    companion object {
        fun unit(): Shape = Square(1.0)
    }
}

enum class Color { RED, GREEN }

interface Visitor {
    fun visit(s: Shape)
}

object Registry {
    private val items = mutableListOf<Shape>()
    fun register(s: Shape) { items.add(s) }
}

typealias Handler = (Shape) -> Unit

fun `runs a test`() {
    check(true)
}
"##;

#[test]
fn kotlin_functions_classes_and_members() {
    let all = find_symbols(KOTLIN, Lang::Kotlin);
    check(&all, "add", Function, None, 6, 12);
    assert_eq!(one(&all, "add").lead, 5);
    check(&all, "shout", Function, None, 14, 14);
    check(&all, "second", Function, None, 16, 17);
    check(&all, "typed", Function, None, 19, 19);
    check(&all, "Point", Class, None, 21, 21);
    check(&all, "Shape", Class, None, 23, 35);
    check(&all, "area", Method, Some("Shape"), 26, 26);
    check(&all, "compareTo", Method, Some("Shape"), 28, 30);
    check(&all, "unit", Method, Some("Shape"), 33, 33);
    check(&all, "Color", Enum, None, 37, 37);
    check(&all, "Visitor", Interface, None, 39, 41);
    check(&all, "visit", Method, Some("Visitor"), 40, 40);
    check(&all, "Registry", Class, None, 43, 46);
    check(&all, "register", Method, Some("Registry"), 45, 45);
    check(&all, "Handler", Type, None, 48, 48);
    check(&all, "runs a test", Function, None, 50, 52);
    absent(
        &all,
        &[
            "fakeInRawString",
            "fakeInComment",
            "helper",
            "uppercase",
            "max",
            "check",
            "items",
            "mutableListOf",
            "Square",
        ],
    );
}

const SWIFT: &str = r##"import Foundation

/// Adds.
func add(_ a: Int, _ b: Int) -> Int {
    let s = """
    func fakeInString() {
    """
    // func fakeInComment() {}
    return helper(a) + b
}

public final class Cart: Codable {
    private var items: [Item] = []
    static let shared = Cart()

    init(items: [Item]) {
        self.items = items
    }

    @discardableResult
    public func add(_ item: Item) -> Bool {
        items.append(item)
        return true
    }

    class func make() -> Cart { Cart(items: []) }
    private(set) var count: Int = 0
}

struct Item {
    let name: String
    mutating func rename(to name: String) { }
}

enum Status: String {
    case open, closed
}

protocol Priced {
    func price() -> Decimal
}

extension Cart: Priced {
    func price() -> Decimal {
        return 0
    }
}

actor Counter {
    func bump() async { }
}

typealias Handler = (Item) -> Void
"##;

#[test]
fn swift_functions_types_and_members() {
    let all = find_symbols(SWIFT, Lang::Swift);
    let top = one_kind(&all, "add", Function);
    assert_eq!((top.line, top.end_line, top.lead), (4, 10, 3));
    check(&all, "Cart", Class, None, 12, 28);
    check(&all, "init", Method, Some("Cart"), 16, 18);
    let method = one_kind(&all, "add", Method);
    assert_eq!(
        (
            method.container.as_deref(),
            method.line,
            method.end_line,
            method.lead
        ),
        (Some("Cart"), 21, 24, 20)
    );
    check(&all, "make", Method, Some("Cart"), 26, 26);
    check(&all, "Item", Struct, None, 30, 33);
    check(&all, "rename", Method, Some("Item"), 32, 32);
    check(&all, "Status", Enum, None, 35, 37);
    check(&all, "Priced", Interface, None, 39, 41);
    check(&all, "Counter", Class, None, 49, 51);
    check(&all, "bump", Method, Some("Counter"), 50, 50);
    check(&all, "Handler", Type, None, 53, 53);
    let prices: Vec<_> = all
        .iter()
        .filter(|s| &*s.name == "price")
        .map(|s| (s.container.as_deref(), s.line, s.end_line))
        .collect();
    assert_eq!(
        prices,
        vec![(Some("Priced"), 40, 40), (Some("Cart"), 44, 46)]
    );
    let ext: Vec<_> = all.iter().filter(|s| s.kind == Impl).collect();
    assert_eq!(ext.len(), 1);
    assert_eq!(
        (&*ext[0].name, ext[0].line, ext[0].end_line),
        ("Cart", 43, 47)
    );
    absent(
        &all,
        &[
            "fakeInString",
            "fakeInComment",
            "helper",
            "append",
            "items",
            "shared",
            "count",
            "open",
        ],
    );
}

const C: &str = r##"#include <stdio.h>
#define OPEN(x) do { x(); } while (0)
#define MAX(a, b) ((a) > (b) ? (a) : (b))

struct point {
    int x;
    int y;
};

typedef struct {
    int w, h;
} size_t2;

typedef struct node {
    struct node *next;
} node_t;

enum color { RED, GREEN };

static int counter = 0;
int add(int a, int b);
extern void later(void);
static void (*handler)(int) = NULL;
MODULE_LICENSE("GPL");

/* Adds. */
int add(int a, int b)
{
    const char *s = "int fake_in_string(void) {";
    /* int fake_in_comment(void) {} */
    if (a > b) {
        return helper(a);
    }
    return a + b;
}

static inline unsigned long
hash(const char *s)
{
    return 0;
}

struct point *make_point(int x, int y) {
    return NULL;
}

int main(int argc, char **argv) { return add(1, 2); }
"##;

#[test]
fn c_functions_and_types() {
    let all = find_symbols(C, Lang::C);
    check(&all, "point", Struct, None, 5, 8);
    check(&all, "size_t2", Struct, None, 10, 12);
    check(&all, "node_t", Struct, None, 14, 16);
    check(&all, "color", Enum, None, 18, 18);
    check(&all, "add", Function, None, 27, 35);
    assert_eq!(one(&all, "add").lead, 26);
    check(&all, "hash", Function, None, 38, 41);
    check(&all, "make_point", Function, None, 43, 45);
    check(&all, "main", Function, None, 47, 47);
    absent(
        &all,
        &[
            "OPEN",
            "MAX",
            "later",
            "handler",
            "MODULE_LICENSE",
            "fake_in_string",
            "fake_in_comment",
            "helper",
            "if",
            "counter",
            "node",
        ],
    );
}

const CPP: &str = r##"#include <vector>

namespace shop {

class Cart final : public Base {
public:
    Cart() : total_{0}, items_(4) {}
    explicit Cart(int n);
    ~Cart() override;

    int total() const { return total_; }
    void add(const Item& item);
    bool operator==(const Cart& o) const { return total_ == o.total_; }
    template <typename T>
    T as() const {
        return static_cast<T>(total_);
    }

private:
    int total_ = compute(1);
    std::vector<Item> items_{};
};

struct Item { int price; };
enum class Kind : int { A, B };

Cart::Cart(int n) : total_(n) {
    const char* s = R"x(void fake_in_raw() {)x";
}

Cart::~Cart() {}

void Cart::add(const Item& item)
{
    if (item.price > 0) {
        total_ += helper(item.price);
    }
}

template <class T>
std::vector<T> wrap(T v) {
    return {v};
}

Cart global(1);
int declared(int a);
using Handler = void (*)(int);

}  // namespace shop

extern "C" {
int c_entry(void) { return 0; }
}
"##;

#[test]
fn cpp_classes_functions_and_members() {
    let all = find_symbols(CPP, Lang::Cpp);
    check(&all, "shop", Module, None, 3, 49);
    check(&all, "total", Method, Some("Cart"), 11, 11);
    check(&all, "operator==", Method, Some("Cart"), 13, 13);
    check(&all, "as", Method, Some("Cart"), 15, 17);
    assert_eq!(one(&all, "as").lead, 14, "the template line leads");
    check(&all, "Item", Struct, None, 24, 24);
    check(&all, "Kind", Enum, None, 25, 25);
    check(&all, "~Cart", Method, Some("Cart"), 31, 31);
    check(&all, "add", Method, Some("Cart"), 33, 38);
    check(&all, "wrap", Function, None, 41, 43);
    assert_eq!(one(&all, "wrap").lead, 40);
    check(&all, "Handler", Type, None, 47, 47);
    check(&all, "c_entry", Function, None, 52, 52);

    let cart: Vec<_> = all
        .iter()
        .filter(|s| &*s.name == "Cart")
        .map(|s| (s.kind, s.container.as_deref(), s.line, s.end_line))
        .collect();
    assert_eq!(
        cart,
        vec![
            (Class, None, 5, 22),
            // The inline constructor, then the one defined outside the class.
            (Method, Some("Cart"), 7, 7),
            (Method, Some("Cart"), 27, 29),
        ]
    );
    absent(
        &all,
        &[
            "fake_in_raw",
            "helper",
            "compute",
            "global",
            "declared",
            "if",
            "static_cast",
            "total_",
            "items_",
        ],
    );
}

#[test]
fn a_closing_brace_in_the_first_column_heals_a_drifted_count() {
    // Both `#ifdef` branches open a block; one `}` follows. The `}` in the first column
    // still ends `f`, and `h` is found at file level.
    let text = "int f(int a)\n{\n#ifdef A\n    if (a) {\n#else\n    if (!a) {\n#endif\n        g();\n    }\n}\nint h(void) { return 1; }\n";
    let all = find_symbols(text, Lang::C);
    check(&all, "f", Function, None, 1, 10);
    check(&all, "h", Function, None, 11, 11);
    // A class closed in the first column inside an unindented namespace ends the class
    // only: the namespace goes on and the function after it is still inside it.
    let text = "namespace a {\nclass B {\n    void m() {}\n};\nvoid f() {}\n}\n";
    let all = find_symbols(text, Lang::Cpp);
    check(&all, "a", Module, None, 1, 6);
    check(&all, "B", Class, None, 2, 4);
    check(&all, "m", Method, Some("B"), 3, 3);
    check(&all, "f", Function, None, 5, 5);
}

#[test]
fn a_block_that_never_closes_falls_back_to_forty_lines() {
    let mut text = String::from("fn broken() {\n");
    for i in 0..100 {
        text.push_str(&format!("    let x{i} = {i};\n"));
    }
    let all = find_symbols(&text, Lang::Rust);
    check(&all, "broken", Function, None, 1, 40);
    // Shorter than forty lines: the end of the file.
    let all = find_symbols("fn broken() {\n    let a = 1;\n", Lang::Rust);
    check(&all, "broken", Function, None, 1, 2);
}

#[test]
fn windows_line_endings_change_nothing() {
    let unix = "def a():\n    return 1\n\nclass B:\n    def c(self):\n        pass\n";
    let windows = unix.replace('\n', "\r\n");
    assert_eq!(
        find_symbols(unix, Lang::Python),
        find_symbols(&windows, Lang::Python)
    );
    let unix = "pub fn a() {\n    1;\n}\nstruct B {\n    c: u8,\n}\n";
    let windows = unix.replace('\n', "\r\n");
    assert_eq!(
        find_symbols(unix, Lang::Rust),
        find_symbols(&windows, Lang::Rust)
    );
    check(&find_symbols(&windows, Lang::Rust), "B", Struct, None, 4, 6);
}

#[test]
fn a_file_gives_at_most_two_thousand_definitions() {
    let mut text = String::new();
    for i in 0..(MAX_SYMBOLS_PER_FILE + 50) {
        text.push_str(&format!("fn f{i}() {{}}\n"));
    }
    assert_eq!(find_symbols(&text, Lang::Rust).len(), MAX_SYMBOLS_PER_FILE);
}

#[test]
fn text_that_is_not_ascii_does_not_break_the_finder() {
    let text = "// héllo → wörld\nfn größe(ä: &str) -> usize { ä.len() }\nstruct Ünïcode;\n";
    let all = find_symbols(text, Lang::Rust);
    check(&all, "größe", Function, None, 2, 2);
    check(&all, "Ünïcode", Struct, None, 3, 3);
    assert!(find_symbols("", Lang::Python).is_empty());
    assert!(find_symbols("\n\n", Lang::Go).is_empty());
}

#[test]
fn names_shown_to_the_user() {
    let all = find_symbols(RUST, Lang::Rust);
    let new = one(&all, "new");
    assert_eq!(new.qualified(Lang::Rust), "Point::new");
    let all = find_symbols(TYPESCRIPT, Lang::TypeScript);
    assert_eq!(
        one(&all, "create").qualified(Lang::TypeScript),
        "Store.create"
    );
    assert_eq!(one_fn(&all, "add").qualified(Lang::TypeScript), "add");
    for kind in [
        Function, Method, Class, Struct, Enum, Trait, Interface, Impl, Module, Const, Type,
    ] {
        assert_eq!(SymbolKind::parse(kind.as_str()), Some(kind));
    }
    assert_eq!(SymbolKind::parse("file"), None);
}
