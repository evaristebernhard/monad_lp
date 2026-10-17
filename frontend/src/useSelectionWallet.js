import {useEffect,useRef,useState} from 'react'
import {createPublicClient,http} from 'viem'
import {monadTestnet,monad} from 'viem/chains'
const testnet=createPublicClient({chain:monadTestnet,transport:http('/api/selection/testnet-rpc',{retryCount:0,timeout:20000})})

const mainnet=createPublicClient({chain:monad,transport:http('/api/monad/trade-rpc',{retryCount:0,timeout:20000})})
export default function useSelectionWallet(){
 const [wallets,setWallets]=useState([]),[walletId,setWalletId]=useState('')
 const [account,setAccount]=useState(''),[chain,setChain]=useState(null),[balance,setBalance]=useState(null),[balanceError,setBalanceError]=useState(''),[refresh,setRefresh]=useState(0)
 const provider=wallets.find(w=>w.id===walletId)?.provider,active=useRef(null),connected=useRef(false)
 active.current=provider
 useEffect(()=>{
  const announce=event=>{const {info,provider:p}=event.detail||{};if(!info?.uuid||!p?.request)return;setWallets(old=>old.some(w=>w.id===info.uuid)?old:[...old,{id:info.uuid,name:info.name,provider:p}]);setWalletId(old=>old||info.uuid)}
  window.addEventListener('eip6963:announceProvider',announce)
  window.dispatchEvent(new Event('eip6963:requestProvider'))
  if(window.ethereum?.request){setWallets(old=>old.some(w=>w.provider===window.ethereum)?old:[...old,{id:'injected',name:'浏览器钱包',provider:window.ethereum}]);setWalletId(old=>old||'injected')}
  return()=>window.removeEventListener('eip6963:announceProvider',announce)
 },[])
 useEffect(()=>{
  connected.current=false;setAccount('');setChain(null);setBalance(null)
  if(!provider)return
  let disposed=false
  const accountsChanged=accounts=>{if(!disposed&&connected.current){setAccount(accounts[0]||'');setBalance(null);setBalanceError('')}}
  const chainChanged=id=>{if(!disposed){setChain(Number(id));setBalance(null)}}
  const disconnected=()=>{if(!disposed){connected.current=false;setAccount('');setChain(null);setBalance(null)}}
  provider.on?.('accountsChanged',accountsChanged);provider.on?.('chainChanged',chainChanged);provider.on?.('disconnect',disconnected)
  return()=>{disposed=true;provider.removeListener?.('accountsChanged',accountsChanged);provider.removeListener?.('chainChanged',chainChanged);provider.removeListener?.('disconnect',disconnected)}
 },[provider])
 useEffect(()=>{
  setBalance(null);setBalanceError('')
  if(!account)return
  let disposed=false
  const load=async()=>{try{const value=await (chain===143?mainnet:testnet).getBalance({address:account});if(!disposed){setBalance(value);setBalanceError('')}}catch{if(!disposed){setBalance(null);setBalanceError('余额读取失败')}}}
  load();const timer=setInterval(load,20000)
  return()=>{disposed=true;clearInterval(timer)}
 },[account,chain,refresh])
 async function connect(){
  const p=provider;if(!p)throw Error('请先安装或选择 EVM 钱包')
  const accounts=await p.request({method:'eth_requestAccounts'}),id=await p.request({method:'eth_chainId'})
  if(active.current!==p)throw Error('钱包已改变，请重新连接')
  if(!accounts[0])throw Error('钱包没有返回可用账户')
  connected.current=true;setAccount(accounts[0]);setChain(Number(id));return accounts[0]
 }
 async function switchTestnet(){
  const p=provider;if(!p)throw Error('请先连接钱包')
  if(Number(await p.request({method:'eth_chainId'}))!==10143){
   try{await p.request({method:'wallet_switchEthereumChain',params:[{chainId:'0x279f'}]})}catch(e){if(Number(e.code)!==4902)throw e;await p.request({method:'wallet_addEthereumChain',params:[{chainId:'0x279f',chainName:'Monad Testnet',nativeCurrency:{name:'MON',symbol:'MON',decimals:18},rpcUrls:['https://testnet-rpc.monad.xyz'],blockExplorerUrls:['https://testnet.monadscan.com']}]});await p.request({method:'wallet_switchEthereumChain',params:[{chainId:'0x279f'}]})}
  }
  if(active.current!==p||Number(await p.request({method:'eth_chainId'}))!==10143)throw Error('钱包网络没有切换到 Monad 测试网')
  const accounts=await p.request({method:'eth_accounts'});setChain(10143);setAccount(accounts[0]||'');setRefresh(n=>n+1)
 }
 function disconnect(){connected.current=false;setAccount('');setChain(null);setBalance(null);setBalanceError('')}
 return {wallets,walletId,setWalletId,provider,account,chain,balance,balanceError,connect,switchTestnet,disconnect,refreshBalance:()=>setRefresh(n=>n+1)}
}
