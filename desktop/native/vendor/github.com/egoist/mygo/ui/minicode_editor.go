package ui

// TextSelection exposes rune positions for native caret-aware completion.
func (e *Element) TextSelection() (anchor, caret int) {
	if ed := e.st.editor; ed != nil {
		return ed.anchor, ed.caret
	}
	return 0, 0
}

// ReplaceSelection inserts an application completion as one undoable edit.
// Its return value must be assigned to the string bound to the text input.
func (e *Element) ReplaceSelection(from, to int, value string) string {
	ed := e.st.editor
	if ed == nil || ed.readOnly {
		return ""
	}
	from = max(0, min(from, ed.buf.n))
	to = max(0, min(to, ed.buf.n))
	if from > to {
		from, to = to, from
	}
	ed.record(false)
	ed.replace(from, to, value)
	if ed.area != nil {
		ed.area.reveal = true
	}
	e.st.changed = true
	e.c.rt.consumed = true
	return ed.buf.s
}

// ReserveTextKeys leaves selected editing keys to this input's shortcuts.
// Composition keys remain owned by the platform input method.
func (e *Element) ReserveTextKeys(reserved func(Modifiers, Key) bool) *Element {
	if ed := e.st.editor; ed != nil {
		ed.appKeys = reserved
	}
	return e
}
