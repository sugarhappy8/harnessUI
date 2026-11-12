class Cart {
  constructor() {
    this.lines = [];
  }

  add(name, price, quantity = 1) {
    this.lines.push({ name, price, quantity });
  }

  total(discountPercent = 0) {
    const sum = this.lines.reduce((s, line) => s + line.price * line.quantity, 0);
    return Math.round(sum * (100 - discountPercent)) / 100;
  }

  count() {
    return this.lines.reduce((n, line) => n + line.quantity, 0);
  }
}

module.exports = { Cart };
