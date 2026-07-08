import { helper } from "./helper";
import { Geo } from "./util/geo";
import React from "react";
const cfg = require("./config");

export function main() {
  helper();
  new Geo();
  console.log(React, cfg);
}
