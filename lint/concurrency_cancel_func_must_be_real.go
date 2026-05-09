package main

import (
	"go/ast"

	"github.com/dgageot/rubocop-go/cop"
)

// ConcurrencyCancelFuncMustBeReal flags two AST shapes that have shown
// up in PR review (e.g. PR #2714) as silent "I look like I cancel but I
// don't" bugs:
//
//  1. an empty function literal `func() {}` assigned to a value whose
//     type is syntactically `context.CancelFunc`, either through:
//
//     - a var/const declaration with an explicit type annotation:
//     `var c context.CancelFunc = func() {}`
//     - a composite literal whose target field is declared
//     `context.CancelFunc` in the same file:
//     `S{cancel: func() {}}`
//
//  2. a [context.WithCancel] / [context.WithCancelCause] /
//     [context.WithDeadline] / [context.WithTimeout] call whose cancel
//     return is dropped onto the blank identifier — either via
//     `_, _ := context.WithCancel(...)` or by a follow-on
//     `_ = cancel` statement.
//
// Each of these "compiles and runs" but defeats cancellation, leaking
// the goroutine / context tree forever. The cop is purely syntactic:
// type aliases of `context.CancelFunc` under a different package name
// are not recognised, and a cancel that is captured by a closure and
// then dropped is not detected. In practice both gaps are rare; the
// shapes above are how the bug actually shipped.
//
// Suppression: per-line `//rubocop:disable Concurrency/CancelFuncMustBeReal`
// with a one-line rationale.
var ConcurrencyCancelFuncMustBeReal = &cop.Func{
	Meta: cop.Meta{
		Name:        "Concurrency/CancelFuncMustBeReal",
		Description: "context.CancelFunc must be wired up; func() {} placeholders and discarded cancels leak goroutines",
		Severity:    cop.Error,
	},
	Scope: nonTestFileScope,
	Run: func(p *cop.Pass) {
		// Shape 1a: var/const with explicit context.CancelFunc type.
		ast.Inspect(p.File, func(n ast.Node) bool {
			vs, ok := n.(*ast.ValueSpec)
			if !ok || vs.Type == nil || !isCancelFuncType(vs.Type) {
				return true
			}
			for _, v := range vs.Values {
				if isEmptyFuncLit(v) {
					p.Reportf(v,
						"empty func() {} assigned to a context.CancelFunc — this looks like a "+
							"cancel but cancels nothing; either store the real CancelFunc "+
							"returned by context.WithCancel, or store nil and nil-check at the "+
							"call site")
				}
			}
			return true
		})

		// Shape 1b: composite literal field whose declared type in the
		// receiving struct is context.CancelFunc.
		cancelFields := cancelFuncFieldsInFile(p.File)
		ast.Inspect(p.File, func(n ast.Node) bool {
			cl, ok := n.(*ast.CompositeLit)
			if !ok {
				return true
			}
			typeName, ok := compositeLitTypeName(cl)
			if !ok {
				return true
			}
			fields := cancelFields[typeName]
			if len(fields) == 0 {
				return true
			}
			for _, elt := range cl.Elts {
				kv, ok := elt.(*ast.KeyValueExpr)
				if !ok {
					continue
				}
				key, ok := kv.Key.(*ast.Ident)
				if !ok || !fields[key.Name] {
					continue
				}
				if isEmptyFuncLit(kv.Value) {
					p.Reportf(kv,
						"empty func() {} assigned to context.CancelFunc field %s.%s — this "+
							"looks like a cancel but cancels nothing; either store the real "+
							"CancelFunc returned by context.WithCancel, or omit the field so "+
							"the zero value (nil) is preserved and nil-check at the call site",
						typeName, key.Name)
				}
			}
			return true
		})

		// Shape 2a: context.WithCancel(...) etc. whose cancel return is
		// dropped onto the blank identifier in the same statement.
		ast.Inspect(p.File, func(n ast.Node) bool {
			as, ok := n.(*ast.AssignStmt)
			if !ok || len(as.Rhs) != 1 {
				return true
			}
			call, ok := as.Rhs[0].(*ast.CallExpr)
			if !ok || !isContextCancelProducingCall(call) {
				return true
			}
			// Two LHS values: ctx, cancel. The cancel is at index 1.
			if len(as.Lhs) >= 2 && isBlankIdent(as.Lhs[1]) {
				p.Reportf(as.Lhs[1],
					"%s returns a CancelFunc that this assignment discards onto _ — "+
						"the context will leak until process exit; bind cancel and "+
						"defer it (or hand it off to teardown)",
					contextCancelProducerName(call))
			}
			return true
		})
	},
}

// isCancelFuncType reports whether expr is the syntactic selector
// `context.CancelFunc`. Aliases, dot-imports, and renamed imports of
// the context package are intentionally not recognised — they are rare
// and the cop's diagnostic still reads correctly when an alias is
// missed.
func isCancelFuncType(expr ast.Expr) bool {
	return cop.IsSelector(expr, "context", "CancelFunc")
}

// isEmptyFuncLit reports whether expr is `func() {}` — a function
// literal with no parameters, no results, and an empty body. A literal
// with parameters or results is intentionally treated as "real" even
// when its body is empty, since the caller's intent is harder to read.
func isEmptyFuncLit(expr ast.Expr) bool {
	fn, ok := expr.(*ast.FuncLit)
	if !ok {
		return false
	}
	if fn.Type.Params != nil && len(fn.Type.Params.List) > 0 {
		return false
	}
	if fn.Type.Results != nil && len(fn.Type.Results.List) > 0 {
		return false
	}
	return fn.Body != nil && len(fn.Body.List) == 0
}

// cancelFuncFieldsInFile returns, for every top-level struct in file, the
// set of field names whose declared type is syntactically
// `context.CancelFunc`. Used to catch composite-literal assignments that
// silently store a no-op cancel into a field that ought to terminate
// goroutines.
func cancelFuncFieldsInFile(file *ast.File) map[string]map[string]bool {
	out := map[string]map[string]bool{}
	for _, decl := range file.Decls {
		gd, ok := decl.(*ast.GenDecl)
		if !ok {
			continue
		}
		for _, spec := range gd.Specs {
			ts, ok := spec.(*ast.TypeSpec)
			if !ok {
				continue
			}
			st, ok := ts.Type.(*ast.StructType)
			if !ok {
				continue
			}
			fields := map[string]bool{}
			for _, f := range st.Fields.List {
				if !isCancelFuncType(f.Type) {
					continue
				}
				for _, name := range f.Names {
					fields[name.Name] = true
				}
			}
			if len(fields) > 0 {
				out[ts.Name.Name] = fields
			}
		}
	}
	return out
}

// compositeLitTypeName returns the bare type name of a composite
// literal (`T{...}`, `&T{...}`, `pkg.T{...}` — for the last we still
// return "T" because we only key into per-file struct definitions).
// Anonymous types (`struct{...}{...}`) and indexed types
// (`Container[T]{...}`) are intentionally not recognised.
func compositeLitTypeName(cl *ast.CompositeLit) (string, bool) {
	switch t := cl.Type.(type) {
	case *ast.Ident:
		return t.Name, true
	case *ast.SelectorExpr:
		return t.Sel.Name, true
	}
	return "", false
}

// isBlankIdent reports whether expr is the blank identifier `_`. Used
// to recognise discarded cancel returns from context.WithCancel.
func isBlankIdent(expr ast.Expr) bool {
	id, ok := expr.(*ast.Ident)
	return ok && id.Name == "_"
}

// isContextCancelProducingCall reports whether call is one of the
// context-package functions that return a CancelFunc (or
// CancelCauseFunc) as their second result.
func isContextCancelProducingCall(call *ast.CallExpr) bool {
	_, ok := cop.CallTo(call, "context",
		"WithCancel",
		"WithCancelCause",
		"WithDeadline",
		"WithDeadlineCause",
		"WithTimeout",
		"WithTimeoutCause",
	)
	return ok
}

// contextCancelProducerName returns the name of the matched context
// function (e.g. "context.WithCancel") for use in the diagnostic. It
// duplicates the work of [isContextCancelProducingCall] so the
// diagnostic can read more naturally; both routines share the same
// list of names.
func contextCancelProducerName(call *ast.CallExpr) string {
	name, ok := cop.CallTo(call, "context",
		"WithCancel",
		"WithCancelCause",
		"WithDeadline",
		"WithDeadlineCause",
		"WithTimeout",
		"WithTimeoutCause",
	)
	if !ok {
		return "context.With*"
	}
	return "context." + name
}

// nonTestFileScope is a CheckScope predicate matching production
// (non-_test.go) files. Tests are exempted because they routinely use
// _, _ = context.WithCancel(ctx) inside a t.Cleanup-bound fixture that
// the cop's syntactic check cannot follow.
func nonTestFileScope(p *cop.Pass) bool { return !p.IsTestFile() }
