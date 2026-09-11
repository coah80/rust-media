const elements = new Map();
class Element {
    constructor(id) { this.id = id; this.listeners = []; }
    get textContent() { return __text(this.id); }
    set textContent(value) { __setText(this.id, String(value)); }
    addEventListener(type, callback) {
        if (type !== 'click') throw new Error('Only click listeners are implemented');
        if (this.listeners.length >= 64) throw new Error('Listener limit reached');
        this.listeners.push(callback);
    }
}
globalThis.document = {
    querySelector(selector) {
        const id = __query(String(selector));
        if (id === null) return null;
        if (!elements.has(id)) elements.set(id, new Element(id));
        return elements.get(id);
    }
};
globalThis.__dispatchClick = id => {
    const target = elements.get(id);
    if (target) for (const listener of target.listeners) listener.call(target, {type: 'click', target});
};
