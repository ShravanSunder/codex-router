package parser

import (
	"testing"

	"github.com/charmbracelet/vhs/lexer"
	"github.com/charmbracelet/vhs/token"
)

// The literal function-key command must survive tokenization and parsing.
// Native key delivery is proved by the permanent real-TUI details PNG scenes;
// mapping this command to Enter or Noop cannot satisfy those scene oracles.
func TestCaptureFunctionKeyF4(t *testing.T) {
	expectedType := token.Type("F4")
	if actual := token.LookupIdentifier("F4"); actual != expectedType {
		t.Fatalf("F4 must be a native key token, got %s", actual)
	}
	if !token.IsCommand(expectedType) {
		t.Fatal("F4 must be classified as an executable command")
	}
	parser := New(lexer.New("F4@0ms 2\n"))
	commands := parser.Parse()
	if errors := parser.Errors(); len(errors) != 0 {
		t.Fatalf("F4 must parse through the keypress path: %v", errors)
	}
	if len(commands) != 1 {
		t.Fatalf("expected one native-key command, got %v", commands)
	}
	command := commands[0]
	if command.Type != CommandType(expectedType) || command.Args != "2" || command.Options != "0ms" {
		t.Fatalf("function-key repetition/speed was lost: %+v", command)
	}
}
