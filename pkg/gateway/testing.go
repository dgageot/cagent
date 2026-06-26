package gateway

import "context"

// OverrideCatalogForTesting replaces the catalog loader with a fixed function
// that returns the given catalog. It should only be called from TestMain or
// equivalent test-setup code, before any call to ServerSpec.
func OverrideCatalogForTesting(catalog Catalog) {
	catalogOverride = func(context.Context) (Catalog, error) {
		return catalog, nil
	}
}
