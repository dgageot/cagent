# CRUSH.md - Development Guidelines for cagent

## Build & Test Commands
- `task build` - Build application binary (includes web frontend)
- `task build-web` - Build React frontend only
- `task test` - Run all Go tests (requires web build first)
- `go test ./pkg/servicecore` - Run tests for specific package
- `go test -run TestSpecificFunction ./pkg/agent` - Run single test
- `task lint` - Run golangci-lint with project configuration
- `task link` - Create symlink to ~/bin for easy CLI access

## Code Style Guidelines

### Package Structure & Imports
- Follow standard Go project layout: `pkg/` for libraries, `cmd/` for executables
- Group imports: stdlib, third-party, local packages (separated by blank lines)
- Use consistent import aliases as defined in `.golangci.yml` importas settings
- Package comments required for all public packages (see `pkg/servicecore/types.go`)

### Naming Conventions
- Use camelCase for variables/functions, PascalCase for exported types
- Interface names: end with -er suffix (e.g., `ServiceManager`, `Provider`)
- Constants: ALL_CAPS with underscores (e.g., `DEFAULT_CLIENT_ID`)
- File names: lowercase with underscores (e.g., `service_core.go`)

### Types & Structs
- Define interfaces in separate files when possible
- Use struct embedding for composition over inheritance
- Add struct tags for JSON/YAML serialization consistently
- Document all exported types and methods

### Error Handling
- Return errors as last parameter: `func Do() (result, error)`
- Use `fmt.Errorf` with `%w` verb for error wrapping
- Check errors immediately after function calls
- Use context.Context for cancellation and timeouts

### Multi-tenant Architecture
- All operations must include client ID for isolation (see `pkg/servicecore/`)
- Use `DEFAULT_CLIENT_ID` only for backward compatibility
- Validate client access in all service layer operations