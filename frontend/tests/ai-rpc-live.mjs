// Real RPC only: read contracts and logs without loading a signer or sending transactions.
import fs from 'node:fs'
import assert from 'node:assert/strict'
import {createPublicClient,http} from 'viem'
import {monadTestnet} from 'viem/chains'
import {readMarket,createDecisionReader,sessionAbi} from '../src/ai/chainData.js'

const config=JSON.parse(fs.readFileSync(new URL('../public/ai/session-config.json',import.meta.url),'utf8'))
const url=process.env.MONAD_AI_RPC_URL||'https://testnet-rpc.monad.xyz'
const client=createPublicClient({chain:monadTestnet,transport:http(url,{timeout:25000,retryCount:1})})
const market=await readMarket(client,config)
const wallet=process.env.MONAD_AI_AUDIT_WALLET||config.keeper
const [id,startedAt,,nextRound]=await client.readContract({address:config.contract,abi:sessionAbi,
  functionName:'sessions',args:[wallet]})
const session={id:Number(id),startedAt:Number(startedAt)}
const events=session.id?await createDecisionReader(client)(config,wallet,session):[]
assert(events.every(e=>/^0x[\da-f]{64}$/i.test(e.hash)))
assert.equal(events.length,Number(nextRound),'Every completed round must have an RPC Decision event')
console.log(JSON.stringify({source:url,market,wallet,session,confirmedDecisions:events},null,2))
