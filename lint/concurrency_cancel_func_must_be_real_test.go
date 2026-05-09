package main

import (
	"strings"
	"testing"

	"github.com/dgageot/rubocop-go/coptest"
)

func TestConcurrencyCancelFuncMustBeReal(t *testing.T) {
	t.Parallel()
	cases := []struct {
		name     string
		filename string
		src      string
		want     int
		wantMsg  string
	}{
		{
			name:     "var with explicit context.CancelFunc type and func() {} is reported",
			filename: "pkg/x/x.go",
			src: `package x

import "context"

var noop context.CancelFunc = func() {}
`,
			want:    1,
			wantMsg: "cancels nothing",
		},
		{
			name:     "composite literal field of type context.CancelFunc with func() {} is reported",
			filename: "pkg/x/x.go",
			src: `package x

import "context"

type S struct {
	cancel context.CancelFunc
}

var _ = S{cancel: func() {}}
`,
			want:    1,
			wantMsg: "S.cancel",
		},
		{
			name:     "context.WithCancel with cancel discarded onto _ is reported",
			filename: "pkg/x/x.go",
			src: `package x

import "context"

func F(ctx context.Context) {
	_, _ = context.WithCancel(ctx)
}
`,
			want:    1,
			wantMsg: "context.WithCancel",
		},
		{
			name:     "context.WithTimeout with cancel discarded is also reported",
			filename: "pkg/x/x.go",
			src: `package x

import (
	"context"
	"time"
)

func F(ctx context.Context) {
	_, _ = context.WithTimeout(ctx, time.Second)
}
`,
			want: 1,
		},
		{
			name:     "real CancelFunc binding is OK",
			filename: "pkg/x/x.go",
			src: `package x

import "context"

func F(ctx context.Context) {
	c, cancel := context.WithCancel(ctx)
	defer cancel()
	_ = c
}
`,
			want: 0,
		},
		{
			name:     "func() {} stored under a non-CancelFunc field is OK",
			filename: "pkg/x/x.go",
			src: `package x

type S struct {
	cleanup func()
}

var _ = S{cleanup: func() {}}
`,
			want: 0,
		},
		{
			name:     "var with no type annotation is not flagged (we can't know without types)",
			filename: "pkg/x/x.go",
			src: `package x

var noop = func() {}
`,
			want: 0,
		},
		{
			name:     "test files are exempt",
			filename: "pkg/x/x_test.go",
			src: `package x

import "context"

func F(ctx context.Context) {
	_, _ = context.WithCancel(ctx)
}
`,
			want: 0,
		},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			t.Parallel()
			offenses := coptest.RunNamed(t, ConcurrencyCancelFuncMustBeReal, tc.filename, tc.src)
			if len(offenses) != tc.want {
				t.Fatalf("got %d offenses, want %d:\n%v", len(offenses), tc.want, offenses)
			}
			if tc.wantMsg != "" && !strings.Contains(offenses[0].Message, tc.wantMsg) {
				t.Fatalf("offense message %q does not contain %q", offenses[0].Message, tc.wantMsg)
			}
		})
	}
}
