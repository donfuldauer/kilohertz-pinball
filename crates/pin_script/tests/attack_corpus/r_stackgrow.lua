local function f(n) if n == 0 then return 0 end local a,b,c,d,e,f1,g,h,i,j,k,l,m = 1,2,3,4,5,6,7,8,9,10,11,12,13 return f(n - 1) end
function on_tick() while true do pcall(f, 190) end end
