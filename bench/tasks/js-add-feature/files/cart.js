class Cart {
  constructor() {
    this.lines = [];
  }

  add(name, price, quantity = 1) {
    this.lines.push({ name, price, quantity });
  }

  count() {
    return this.lines.reduce((n, line) => n + line.quantity, 0);
  }
}

module.exports = { Cart };
