import assert from 'node:assert/strict'
import {test} from 'node:test'
import {finalStatus} from "./service.mjs";import fs from "node:fs";test("last status",()=>{const lines=fs.readFileSync("trace.log","utf8").split("\n");assert.equal(finalStatus(lines,"R-17"),"completed");assert.equal(finalStatus(lines,"absent"),null)})
