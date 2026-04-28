"use strict";

const readline = require("node:readline");

class Interface extends readline.Interface {
  question(query) {
    return new Promise((resolve) => super.question(query, resolve));
  }
}

function createInterface(opts) {
  return new Interface(opts);
}

module.exports = { Interface, createInterface };
