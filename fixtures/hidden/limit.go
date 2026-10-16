package hidden

// HiddenLimit is read here and in limit_test.go. The fixture holds no calls,
// so the call graph pinned over fixtures/ stays the same.
const HiddenLimit = 3

var doubled = HiddenLimit * 2
