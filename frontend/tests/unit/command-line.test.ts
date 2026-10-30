/** Registration commands are argv on the chosen machine; shell text stays literal. */
import { expect,it } from 'vitest';
import { commandArguments,commandLine } from '@/lib/commandLine';

it('reads paths and arguments including Windows paths, quotes and empty values',()=>{
  expect(commandArguments('"C:\\Program Files\\agent.exe" --model "local model"')).toEqual(['C:\\Program Files\\agent.exe','--model','local model']);
  expect(commandArguments("'/opt/my agent' --name 'Ada\"s model' ''")).toEqual(['/opt/my agent','--name','Ada"s model','']);
  expect(commandArguments('adapter --key $(cat secret); rm -rf /')).toEqual(['adapter','--key','$(cat','secret);','rm','-rf','/']);
  expect(commandArguments('  ')).toEqual([]);
  expect(commandArguments('"unfinished')).toBeNull();expect(commandArguments('adapter\0arg')).toBeNull();
});
it('displays commands that round-trip every argument, without shell expansion',()=>{
  for(const args of [['/opt/agent','--config','a b',''],['C:\\tools\\agent.exe',"a'b",'a"b','line\nbreak','$HOME',';']]) {
    expect(commandArguments(commandLine(args))).toEqual(args);
  }
});
