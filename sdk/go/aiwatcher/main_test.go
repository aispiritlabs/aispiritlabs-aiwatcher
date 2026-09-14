package aiwatcher_test

import (
	"testing"

	"go.uber.org/goleak"
)

// The HTTP transport owns a goroutine; a test that forgets to close one, or a
// Close that returns before its goroutine has, fails the package here.
func TestMain(m *testing.M) {
	goleak.VerifyTestMain(m)
}
