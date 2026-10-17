import fs from 'node:fs'
const root=new URL('./',import.meta.url),read=p=>JSON.parse(fs.readFileSync(new URL(p,root),'utf8'))
const contract=name=>{const a=read(`contracts/out/${name}.sol/${name}.json`);return {abi:a.abi,bytecode:a.bytecode.object,runtime:a.deployedBytecode.object,immutableReferences:a.deployedBytecode.immutableReferences}}
const output={sample:read('monad_pools_example.json'),snapshot:read('results/live-v4-selection.json'),routes:read('results/route-catalog.json'),v4:{...contract('MonUsdcRouteSelector'),reference:read('results/route-testnet-deployment.json')},dcar:{...contract('DcarV2Router'),reference:fs.existsSync(new URL('results/dcar-testnet-deployment.json',root))?read('results/dcar-testnet-deployment.json'):null}}
output.paid={abi:contract('PaidMonUsdcRouteSelector').abi,reference:read('results/paid-testnet-deployment.json')}
output.market={abi:contract('StrategyMarket').abi,reference:read('results/market-testnet-deployment.json')}
fs.mkdirSync(new URL('../frontend/src/generated',root),{recursive:true});fs.writeFileSync(new URL('../frontend/src/generated/selection.json',root),JSON.stringify(output))
console.log('Generated public contract artifacts and snapshots; no credentials included')
