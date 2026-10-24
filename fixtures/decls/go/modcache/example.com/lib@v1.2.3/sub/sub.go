package sub

import (
	"example.com/lib"
	"github.com/BigCorp/thing"
	"github.com/BigCorp/thing/v2"
)

// widgets is the package clause of github.com/BigCorp/thing.
func Wrap(w widgets.Widget) lib.Result { return lib.Result{} }

// thing/v2 is not in the cache, so its name falls back to the path.
func Plain() thing.Node { return thing.Node{} }
