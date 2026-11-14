local function it() return 1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20 end
function on_tick() local n = 0 while true do for a,b,c,d,e,f,g,h,i,j,k,l,m,o,p,q,r,s,u,v in it do n = n + 1 if n % 2 == 0 then break end end end end
